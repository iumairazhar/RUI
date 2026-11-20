//! Skills: SKILL.md files that tell a team member how to work with one stack.
//!
//! Starter skills live in the source tree (`skills/`) and are copied to
//! `<data_dir>/skills` on startup, only for folders that are missing.
//! Frontmatter is parsed by hand; unknown keys and nested lines are ignored.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One parsed skill.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Bash patterns the member may run, e.g. `uv *`.
    pub bash: Vec<String>,
    pub run_hint: Option<String>,
    pub license: Option<String>,
    /// `source` inside `metadata: {...}`, usually a URL.
    pub source: Option<String>,
    pub body: String,
}

/// Splits `---` frontmatter from the body.
pub(crate) fn split_frontmatter(text: &str) -> Result<(&str, &str)> {
    let rest = text.strip_prefix("---\n").context("missing frontmatter")?;
    let end = rest.find("\n---").context("unterminated frontmatter")?;
    let body = rest[end + 4..].trim_start_matches('\n');
    Ok((&rest[..end], body))
}

pub(crate) fn field<'a>(front: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    front.lines().find_map(|l| l.strip_prefix(&prefix)).map(str::trim)
}

/// The `source` value inside a one-line `metadata: {source: "...", ...}`.
fn metadata_source(front: &str) -> Option<String> {
    let meta = field(front, "metadata")?;
    let rest = meta[meta.find("source:")? + "source:".len()..].trim_start();
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => rest.split([',', '}']).next()?.trim(),
    };
    Some(value.to_string()).filter(|v| !v.is_empty())
}

/// Parses SKILL.md text.
pub fn parse(text: &str) -> Result<Skill> {
    let (front, body) = split_frontmatter(text)?;
    let name = field(front, "name").filter(|v| !v.is_empty()).context("missing name")?;
    let description = field(front, "description").context("missing description")?;
    let bash = match field(front, "bash") {
        Some(v) => serde_json::from_str(v).context("bash must be a JSON array of strings")?,
        None => Vec::new(),
    };
    if body.trim().is_empty() {
        bail!("empty skill body");
    }
    Ok(Skill {
        name: name.into(),
        description: description.into(),
        bash,
        run_hint: field(front, "run_hint").filter(|v| !v.is_empty()).map(String::from),
        license: field(front, "license").filter(|v| !v.is_empty()).map(String::from),
        source: metadata_source(front),
        body: body.trim_end().into(),
    })
}

fn skills_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

/// Copies `src` into `dst` recursively, creating folders as needed.
pub(crate) fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies each top-level folder of `src_root` into `dst_root`, only if it is missing there.
pub fn seed_from(src_root: &Path, dst_root: &Path) {
    let Ok(entries) = std::fs::read_dir(src_root) else { return };
    for entry in entries.flatten() {
        let target = dst_root.join(entry.file_name());
        if !entry.path().is_dir() || target.exists() {
            continue;
        }
        if let Err(e) = copy_dir(&entry.path(), &target) {
            eprintln!("cannot seed {}: {e}", target.display());
        }
    }
}

/// Marks that the built-in skills were seeded once; skills the owner removes later stay removed.
const SEEDED_MARK: &str = ".seeded";

/// Seeds `<data_dir>/skills` from the source tree's `skills/` folder, on the first run only.
pub fn seed(data_dir: &Path) {
    let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/skills"));
    seed_once(src, &skills_dir(data_dir));
}

/// Seeds `dst_root` unless it was seeded before, then leaves the mark.
pub fn seed_once(src_root: &Path, dst_root: &Path) {
    let mark = dst_root.join(SEEDED_MARK);
    if mark.exists() {
        return;
    }
    seed_from(src_root, dst_root);
    let _ = std::fs::create_dir_all(dst_root).and_then(|_| std::fs::write(&mark, ""));
}

fn git_common_dir(wt: &Path) -> Result<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(wt)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!("not a git repository: {}", wt.display());
    }
    let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    Ok(if dir.is_absolute() { dir } else { wt.join(dir) })
}

const SKILL_DIRS: [&str; 2] = [".claude/skills", ".agents/skills"];

/// Adds both installed-skill folders to the repo's `info/exclude`, each only once.
fn exclude_installed_skills(wt: &Path) -> Result<()> {
    let info = git_common_dir(wt)?.join("info");
    std::fs::create_dir_all(&info)?;
    let file = info.join("exclude");
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    for dir in SKILL_DIRS {
        let line = format!("{dir}/");
        if text.lines().any(|l| l.trim() == line) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("{line}\n"));
    }
    std::fs::write(&file, text)?;
    Ok(())
}

/// The named skills that exist and are switched on.
pub fn installable(data_dir: &Path, names: &[String]) -> Vec<String> {
    let on = |n: &&String| crate::library::is_on(data_dir, crate::library::Kind::Skill, n);
    names.iter().filter(on).cloned().collect()
}

/// Names of switched-on skills attached to the client by name.
pub fn client_skills(data_dir: &Path, client: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(skills_dir(data_dir)) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| crate::library::read_meta(&e.path()).attach.clients.iter().any(|c| c == client))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    installable(data_dir, &names)
}

/// Copies the named skills into `<wt>/.claude/skills/` and `<wt>/.agents/skills/`, replacing what is there,
/// and keeps both folders out of git. Skills that are missing or switched off are skipped.
pub fn install_skills(wt: &Path, data_dir: &Path, names: &[String]) -> Result<()> {
    for name in installable(data_dir, names) {
        let src = skills_dir(data_dir).join(&name);
        for dir in SKILL_DIRS {
            let dst = wt.join(dir).join(&name);
            if dst.exists() {
                std::fs::remove_dir_all(&dst)?;
            }
            copy_dir(&src, &dst)?;
        }
    }
    exclude_installed_skills(wt)
}

/// All parsable skills on disk, sorted by name.
pub fn load_all(data_dir: &Path) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(skills_dir(data_dir)) else { return Vec::new() };
    let mut skills: Vec<Skill> = entries
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("SKILL.md")).ok())
        .filter_map(|text| parse(&text).ok())
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// One skill by name.
pub fn load(data_dir: &Path, name: &str) -> Option<Skill> {
    load_all(data_dir).into_iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_HINT: &str = "---\nname: x\ndescription: d\nbash: [\"a *\", \"b *\"]\nrun_hint: run $PORT\n---\nBody line\n";

    #[test]
    fn parses_with_run_hint() {
        let s = parse(WITH_HINT).unwrap();
        assert_eq!(s.name, "x");
        assert_eq!(s.bash, ["a *", "b *"]);
        assert_eq!(s.run_hint.as_deref(), Some("run $PORT"));
        assert_eq!(s.body, "Body line");
    }

    #[test]
    fn parses_without_run_hint() {
        let s = parse("---\nname: y\ndescription: d\nbash: []\n---\nHi\n").unwrap();
        assert_eq!(s.run_hint, None);
        assert!(s.bash.is_empty());
    }

    #[test]
    fn bad_frontmatter_errors() {
        assert!(parse("no frontmatter").is_err());
        assert!(parse("---\nname: x\n---\nbody").is_err());
        assert!(parse("---\nname: x\ndescription: d\nbash: nope\n---\nbody").is_err());
    }

    #[test]
    fn extra_frontmatter_keys_are_ignored() {
        let text = "---\nname: z\ndescription: d\nlicense: MIT\nmetadata:\n  author: me\n  name: other\n---\nBody\n";
        let s = parse(text).unwrap();
        assert_eq!((s.name.as_str(), s.description.as_str()), ("z", "d"));
        assert!(s.bash.is_empty());
    }

    #[test]
    fn parses_license_and_metadata_source() {
        let text = "---\nname: z\ndescription: d\nlicense: Apache-2.0\nmetadata: {source: \"https://github.com/a/b/blob/main/x.md\", adapted: true}\n---\nBody\n";
        let s = parse(text).unwrap();
        assert_eq!(s.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(s.source.as_deref(), Some("https://github.com/a/b/blob/main/x.md"));
        let bare = parse("---\nname: z\ndescription: d\nmetadata: {source: Proprietary-RUI, adapted: false}\n---\nB\n").unwrap();
        assert_eq!(bare.source.as_deref(), Some("Proprietary-RUI"));
        let none = parse("---\nname: z\ndescription: d\n---\nB\n").unwrap();
        assert_eq!((none.license, none.source), (None, None));
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn seed_copies_folders_only_when_missing() {
        let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(&src.path().join("a/SKILL.md"), WITH_HINT);
        write(&src.path().join("a/refs/more.md"), "more");
        write(&src.path().join("b/SKILL.md"), "---\nname: b\ndescription: d\n---\nB\n");
        write(&dst.path().join("b/SKILL.md"), "edited");
        seed_from(src.path(), dst.path());
        assert_eq!(std::fs::read_to_string(dst.path().join("a/refs/more.md")).unwrap(), "more");
        assert_eq!(std::fs::read_to_string(dst.path().join("b/SKILL.md")).unwrap(), "edited");
        write(&dst.path().join("a/SKILL.md"), "edited too");
        seed_from(src.path(), dst.path());
        assert_eq!(std::fs::read_to_string(dst.path().join("a/SKILL.md")).unwrap(), "edited too");
    }

    #[test]
    fn seed_once_lets_removed_skills_stay_removed() {
        let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(&src.path().join("a/SKILL.md"), WITH_HINT);
        seed_once(src.path(), dst.path());
        assert!(dst.path().join("a/SKILL.md").exists());
        std::fs::remove_dir_all(dst.path().join("a")).unwrap();
        seed_once(src.path(), dst.path());
        assert!(!dst.path().join("a").exists());
    }

    #[test]
    fn load_reads_seeded_skills() {
        let d = tempfile::tempdir().unwrap();
        write(&d.path().join("skills/tester/SKILL.md"), WITH_HINT.replace("name: x", "name: tester").as_str());
        assert_eq!(load_all(d.path()).len(), 1);
        assert_eq!(load(d.path(), "tester").unwrap().body, "Body line");
        assert!(load(d.path(), "nope").is_none());
    }

    fn git_repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new("git").arg("init").arg(d.path()).output().unwrap().status.success();
        assert!(ok);
        d
    }

    #[test]
    fn off_skills_are_not_installed_and_client_skills_are() {
        let (data, repo) = (tempfile::tempdir().unwrap(), git_repo());
        write(&data.path().join("skills/on1/SKILL.md"), "x");
        write(&data.path().join("skills/off1/SKILL.md"), "x");
        write(&data.path().join("skills/off1/.rui.json"), "{\"status\":\"off\"}");
        write(&data.path().join("skills/cl/SKILL.md"), "x");
        write(&data.path().join("skills/cl/.rui.json"), "{\"attach\":{\"clients\":[\"acme\"]}}");
        write(&data.path().join("skills/cloff/SKILL.md"), "x");
        write(&data.path().join("skills/cloff/.rui.json"), "{\"status\":\"off\",\"attach\":{\"clients\":[\"acme\"]}}");
        assert_eq!(client_skills(data.path(), "acme"), ["cl"]);
        assert!(client_skills(data.path(), "other").is_empty());
        let names = vec!["on1".to_string(), "off1".to_string()];
        install_skills(repo.path(), data.path(), &names).unwrap();
        assert!(repo.path().join(".claude/skills/on1/SKILL.md").exists());
        assert!(!repo.path().join(".claude/skills/off1").exists());
    }

    #[test]
    fn install_copies_replaces_and_is_not_committed() {
        let (data, repo) = (tempfile::tempdir().unwrap(), git_repo());
        write(&data.path().join("skills/s1/SKILL.md"), "v1");
        write(&data.path().join("skills/s1/extra/x.txt"), "x");
        let names = vec!["s1".to_string(), "missing".to_string()];
        install_skills(repo.path(), data.path(), &names).unwrap();
        let installed = repo.path().join(".claude/skills/s1");
        assert_eq!(std::fs::read_to_string(installed.join("extra/x.txt")).unwrap(), "x");
        write(&data.path().join("skills/s1/SKILL.md"), "v2");
        std::fs::remove_file(data.path().join("skills/s1/extra/x.txt")).unwrap();
        install_skills(repo.path(), data.path(), &names).unwrap();
        assert_eq!(std::fs::read_to_string(installed.join("SKILL.md")).unwrap(), "v2");
        assert!(!installed.join("extra/x.txt").exists());
        write(&repo.path().join("code.txt"), "c");
        assert!(crate::worktree::commit_all(repo.path(), "m").unwrap());
        let files = std::process::Command::new("git").arg("-C").arg(repo.path()).args(["ls-files"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&files.stdout).trim(), "code.txt");
        let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches(".claude/skills/").count(), 1);
        assert_eq!(exclude.matches(".agents/skills/").count(), 1);
        assert!(repo.path().join(".agents/skills/s1/SKILL.md").exists());
        assert!(repo.path().join(".claude/skills/s1/SKILL.md").exists());
    }

    #[test]
    fn installed_skills_do_not_show_in_git_status() {
        let (data, repo) = (tempfile::tempdir().unwrap(), git_repo());
        write(&data.path().join("skills/s1/SKILL.md"), "v1");
        install_skills(repo.path(), data.path(), &["s1".to_string()]).unwrap();
        let out = std::process::Command::new("git").arg("-C").arg(repo.path()).args(["status", "--porcelain"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");
    }
}

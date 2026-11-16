//! Shared helpers for the skill and role library: sidecar metadata, history, trash and frontmatter edits.
//!
//! Every skill or role folder may hold a `.rui.json` sidecar (status, author, hist, ...) and a
//! `.history/<unix_ms>.md` snapshot per old version. A missing sidecar means shipped content: on / shipped.

use crate::{roster, skills};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SIDECAR: &str = ".rui.json";
const HISTORY: &str = ".history";

/// What kind of library item a folder holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Skill,
    Role,
}

impl Kind {
    pub fn folder(self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Role => "roles",
        }
    }

    pub fn file(self) -> &'static str {
        match self {
            Kind::Skill => "SKILL.md",
            Kind::Role => "AGENT.md",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Role => "role",
        }
    }
}

/// Error with an HTTP-shaped meaning.
#[derive(Debug)]
pub enum LibError {
    Bad(String),
    NotFound(String),
    Conflict(String),
    Other(anyhow::Error),
}

pub type LibResult<T> = Result<T, LibError>;

impl From<anyhow::Error> for LibError {
    fn from(e: anyhow::Error) -> Self {
        LibError::Other(e)
    }
}

impl From<std::io::Error> for LibError {
    fn from(e: std::io::Error) -> Self {
        LibError::Other(e.into())
    }
}

impl From<serde_json::Error> for LibError {
    fn from(e: serde_json::Error) -> Self {
        LibError::Other(e.into())
    }
}

pub fn bad<T>(msg: impl Into<String>) -> LibResult<T> {
    Err(LibError::Bad(msg.into()))
}

/// Clients a skill is attached to (role attachment lives in each role's `skills:` list).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Attach {
    #[serde(default)]
    pub clients: Vec<String>,
}

/// One history line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hist {
    pub when: String,
    pub who: String,
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// The `.rui.json` sidecar. Roles only use status, author and hist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_author")]
    pub author: String,
    #[serde(default, skip_serializing_if = "attach_empty")]
    pub attach: Attach,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub hist: Vec<Hist>,
}

fn default_status() -> String {
    "on".into()
}

fn default_author() -> String {
    "shipped".into()
}

fn attach_empty(a: &Attach) -> bool {
    a.clients.is_empty()
}

impl Default for Meta {
    fn default() -> Self {
        Meta { status: default_status(), author: default_author(), attach: Attach::default(), tools: Vec::new(), hist: Vec::new() }
    }
}

impl Meta {
    /// A fresh sidecar for something the owner made or brought in.
    pub fn new(status: &str, author: &str) -> Meta {
        Meta { status: status.into(), author: author.into(), ..Meta::default() }
    }

    /// Appends one history line by "You".
    pub fn log(&mut self, what: &str, why: Option<&str>, from: Option<&str>) {
        let clean = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        self.hist.push(Hist {
            when: stamp(now_ms()),
            who: "You".into(),
            what: what.into(),
            why: why.and_then(clean),
            from: from.and_then(clean),
        });
    }
}

/// Reads the sidecar in `dir`; missing or broken means defaults.
pub fn read_meta(dir: &Path) -> Meta {
    let Ok(text) = std::fs::read_to_string(dir.join(SIDECAR)) else { return Meta::default() };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_meta(dir: &Path, meta: &Meta) -> LibResult<()> {
    std::fs::write(dir.join(SIDECAR), serde_json::to_string_pretty(meta)?)?;
    Ok(())
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `YYYY-MM-DD` (UTC) of a unix millisecond timestamp.
pub fn date_of(ms: u64) -> String {
    let (y, m, d) = civil((ms / 86_400_000) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD HH:MM` (UTC).
pub fn stamp(ms: u64) -> String {
    let mins = ms / 60_000 % 1440;
    format!("{} {:02}:{:02}", date_of(ms), mins / 60, mins % 60)
}

/// Names are lowercase letters, digits and hyphens, at most 64 characters.
pub fn valid_name(name: &str) -> LibResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        return Ok(());
    }
    bad("name must be lowercase letters, digits and hyphens, at most 64 characters")
}

pub fn kind_dir(data_dir: &Path, kind: Kind) -> PathBuf {
    data_dir.join(kind.folder())
}

/// The folder of an existing item. Rejects unsafe names.
pub fn item_dir(data_dir: &Path, kind: Kind, name: &str) -> LibResult<PathBuf> {
    valid_name(name)?;
    let dir = kind_dir(data_dir, kind).join(name);
    if !dir.join(kind.file()).is_file() {
        return Err(LibError::NotFound(format!("{} '{name}' not found", kind.label())));
    }
    Ok(dir)
}

/// Whether the item is switched on (missing items are not).
pub fn is_on(data_dir: &Path, kind: Kind, name: &str) -> bool {
    item_dir(data_dir, kind, name).map(|d| read_meta(&d).status == "on").unwrap_or(false)
}

/// Checks that `text` parses as this kind of file and names `name`.
pub fn validate(kind: Kind, name: &str, text: &str) -> LibResult<()> {
    let found = match kind {
        Kind::Skill => skills::parse(text).map(|s| s.name),
        Kind::Role => roster::parse(text).map(|r| r.name),
    };
    match found {
        Err(e) => bad(format!("{e:#}")),
        Ok(n) if n != name => bad(format!("name '{n}' must match the folder '{name}'")),
        Ok(_) => Ok(()),
    }
}

/// Saves `old` as `.history/<unix_ms>.md`.
fn snapshot(dir: &Path, old: &str) -> LibResult<()> {
    let hist = dir.join(HISTORY);
    std::fs::create_dir_all(&hist)?;
    let mut ms = now_ms();
    while hist.join(format!("{ms}.md")).exists() {
        ms += 1;
    }
    std::fs::write(hist.join(format!("{ms}.md")), old)?;
    Ok(())
}

/// Saved version timestamps, newest first.
pub fn versions(dir: &Path) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(dir.join(HISTORY)) else { return Vec::new() };
    let mut ts: Vec<u64> = entries
        .flatten()
        .filter_map(|e| e.path().file_stem()?.to_str()?.parse().ok())
        .collect();
    ts.sort_unstable_by(|a, b| b.cmp(a));
    ts
}

/// Snapshots the current file, writes `text`, and logs one history line.
pub fn save(data_dir: &Path, kind: Kind, name: &str, text: &str, what: &str, why: Option<&str>) -> LibResult<()> {
    let dir = item_dir(data_dir, kind, name)?;
    let file = dir.join(kind.file());
    snapshot(&dir, &std::fs::read_to_string(&file)?)?;
    std::fs::write(&file, text)?;
    let mut meta = read_meta(&dir);
    meta.log(what, why, None);
    write_meta(&dir, &meta)
}

/// Validated edit of the whole file.
pub fn edit(data_dir: &Path, kind: Kind, name: &str, text: &str, why: Option<&str>) -> LibResult<()> {
    item_dir(data_dir, kind, name)?;
    validate(kind, name, text)?;
    save(data_dir, kind, name, text, "edited it", why)
}

/// Brings back a saved version as a new edit.
pub fn restore(data_dir: &Path, kind: Kind, name: &str, ts: u64) -> LibResult<()> {
    let dir = item_dir(data_dir, kind, name)?;
    let Ok(old) = std::fs::read_to_string(dir.join(HISTORY).join(format!("{ts}.md"))) else {
        return Err(LibError::NotFound("version not found".into()));
    };
    validate(kind, name, &old)?;
    save(data_dir, kind, name, &old, &format!("restored the {} version", date_of(ts)), None)
}

/// Replaces or inserts one top-level frontmatter line; every other byte stays the same.
pub fn set_key(text: &str, key: &str, value: &str) -> LibResult<String> {
    let Some(rest) = text.strip_prefix("---\n") else { return bad("missing frontmatter") };
    let Some(end) = rest.find("\n---") else { return bad("unterminated frontmatter") };
    let prefix = format!("{key}:");
    let line = format!("{key}: {value}");
    let mut lines: Vec<&str> = rest[..end].split('\n').collect();
    match lines.iter().position(|l| l.starts_with(&prefix)) {
        Some(i) => lines[i] = &line,
        None => lines.push(&line),
    }
    Ok(format!("---\n{}{}", lines.join("\n"), &rest[end..]))
}

/// Rewrites one frontmatter key of an item, validates the result and saves it.
pub fn rewrite_key(data_dir: &Path, kind: Kind, name: &str, key: &str, value: &str, what: &str) -> LibResult<()> {
    let dir = item_dir(data_dir, kind, name)?;
    let new = set_key(&std::fs::read_to_string(dir.join(kind.file()))?, key, value)?;
    validate(kind, name, &new)?;
    save(data_dir, kind, name, &new, what, None)
}

/// Sets on / off / review and logs it. Turning the lead off is refused.
pub fn set_status(data_dir: &Path, kind: Kind, name: &str, status: &str) -> LibResult<()> {
    let dir = item_dir(data_dir, kind, name)?;
    if !["on", "off", "review"].contains(&status) {
        return bad("status must be on, off or review");
    }
    if kind == Kind::Role && name == "lead" && status != "on" {
        return bad("the lead role cannot be turned off");
    }
    let mut meta = read_meta(&dir);
    let what = match (meta.status.as_str(), status) {
        ("review", "on") => "approved it",
        (_, "on") => "turned it on",
        (_, "off") => "turned it off",
        _ => "sent it to review",
    };
    meta.status = status.into();
    meta.log(what, None, None);
    write_meta(&dir, &meta)
}

/// Moves an item to `<data_dir>/trash/<prefix><name>-<ms>`.
pub fn trash(data_dir: &Path, kind: Kind, name: &str) -> LibResult<PathBuf> {
    let dir = item_dir(data_dir, kind, name)?;
    let prefix = if kind == Kind::Role { "role-" } else { "" };
    let dest = data_dir.join("trash").join(format!("{prefix}{name}-{}", now_ms()));
    std::fs::create_dir_all(data_dir.join("trash"))?;
    std::fs::rename(dir, &dest)?;
    Ok(dest)
}

/// Copies a folder, skipping symlinks, `.git`, sidecars and history (nothing untrusted comes along).
pub fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if [".git", SIDECAR, HISTORY].contains(&name.to_string_lossy().as_ref()) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &dst.join(&name))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dst.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: demo\ndescription: d\nmetadata:\n  x: y\n---\nBody\n";

    pub fn skill_dir(data: &Path, name: &str, text: &str) -> PathBuf {
        let dir = data.join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), text).unwrap();
        dir
    }

    #[test]
    fn sidecar_defaults_when_missing() {
        let d = tempfile::tempdir().unwrap();
        let dir = skill_dir(d.path(), "demo", SKILL);
        let m = read_meta(&dir);
        assert_eq!((m.status.as_str(), m.author.as_str()), ("on", "shipped"));
        assert!(m.attach.clients.is_empty() && m.tools.is_empty() && m.hist.is_empty());
        std::fs::write(dir.join(SIDECAR), "{\"status\":\"off\"}").unwrap();
        assert_eq!((read_meta(&dir).status.as_str(), read_meta(&dir).author.as_str()), ("off", "shipped"));
        std::fs::write(dir.join(SIDECAR), "not json").unwrap();
        assert_eq!(read_meta(&dir), Meta::default());
    }

    #[test]
    fn edit_history_restore() {
        let d = tempfile::tempdir().unwrap();
        let dir = skill_dir(d.path(), "demo", SKILL);
        let v2 = SKILL.replace("Body", "Better body");
        edit(d.path(), Kind::Skill, "demo", &v2, Some("clearer")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), v2);
        let ts = versions(&dir);
        assert_eq!(ts.len(), 1);
        let m = read_meta(&dir);
        assert_eq!((m.hist[0].what.as_str(), m.hist[0].why.as_deref(), m.hist[0].who.as_str()), ("edited it", Some("clearer"), "You"));
        restore(d.path(), Kind::Skill, "demo", ts[0]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), SKILL);
        assert_eq!(versions(&dir).len(), 2);
        let last = read_meta(&dir).hist.pop().unwrap();
        assert_eq!(last.what, format!("restored the {} version", date_of(ts[0])));
        assert!(matches!(restore(d.path(), Kind::Skill, "demo", 1), Err(LibError::NotFound(_))));
    }

    #[test]
    fn validation_errors() {
        let d = tempfile::tempdir().unwrap();
        skill_dir(d.path(), "demo", SKILL);
        let other = SKILL.replace("name: demo", "name: other");
        assert!(matches!(edit(d.path(), Kind::Skill, "demo", &other, None), Err(LibError::Bad(_))));
        assert!(matches!(edit(d.path(), Kind::Skill, "demo", "junk", None), Err(LibError::Bad(_))));
        assert!(matches!(edit(d.path(), Kind::Skill, "nope", SKILL, None), Err(LibError::NotFound(_))));
        for n in ["../x", "a/b", "", "Up", "-a", &"a".repeat(65)] {
            assert!(valid_name(n).is_err(), "{n}");
            assert!(item_dir(d.path(), Kind::Skill, n).is_err(), "{n}");
        }
        assert!(valid_name("ok-name-2").is_ok());
    }

    #[test]
    fn set_key_keeps_other_bytes() {
        let text = "---\nname: x\r\nskills: [\"a\"]\nmetadata:\n  skills: keep\n---\nBody\n\n  end\n";
        let out = set_key(text, "skills", "[\"a\",\"b\"]").unwrap();
        assert_eq!(out, "---\nname: x\r\nskills: [\"a\",\"b\"]\nmetadata:\n  skills: keep\n---\nBody\n\n  end\n");
        let added = set_key(text, "max_loops", "7").unwrap();
        assert_eq!(added, text.replace("  skills: keep\n---", "  skills: keep\nmax_loops: 7\n---"));
        assert!(set_key("no front", "a", "b").is_err());
    }

    #[test]
    fn status_logs_and_trash_moves() {
        let d = tempfile::tempdir().unwrap();
        let dir = skill_dir(d.path(), "demo", SKILL);
        set_status(d.path(), Kind::Skill, "demo", "off").unwrap();
        assert!(!is_on(d.path(), Kind::Skill, "demo"));
        set_status(d.path(), Kind::Skill, "demo", "review").unwrap();
        set_status(d.path(), Kind::Skill, "demo", "on").unwrap();
        let whats: Vec<String> = read_meta(&dir).hist.into_iter().map(|h| h.what).collect();
        assert_eq!(whats, ["turned it off", "sent it to review", "approved it"]);
        assert!(set_status(d.path(), Kind::Skill, "demo", "weird").is_err());
        let dest = trash(d.path(), Kind::Skill, "demo").unwrap();
        assert!(dest.join("SKILL.md").exists() && !dir.exists());
        assert!(dest.starts_with(d.path().join("trash")));
    }

    #[test]
    fn dates_format() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(stamp(1_700_000_000_000), "2023-11-14 22:13");
    }
}

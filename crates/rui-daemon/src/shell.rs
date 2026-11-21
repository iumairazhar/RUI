//! Shell command safety for Rui's Bash tool, and the owner-approval files that release held commands.

use crate::db::Db;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Approvals older than this many seconds are ignored.
const APPROVAL_MAX_AGE_SECS: u64 = 3600;

/// Words that only wrap the real command; they are skipped to find it.
const WRAPPERS: [&str; 16] = [
    "env", "xargs", "nohup", "time", "timeout", "nice", "caffeinate", "watch", "command", "exec", "then", "do", "else", "if",
    "while", "!",
];

/// Returns a short risk reason when the command could change things, or None when it only reads.
pub fn danger(cmd: &str) -> Option<&'static str> {
    split_commands(cmd).iter().find_map(|pipeline| pipeline_danger(pipeline))
}

/// Splits a command line into pipelines; each pipeline is its parts split on `|`.
/// Subshells, `$(...)` and backticks become separate commands.
fn split_commands(cmd: &str) -> Vec<Vec<String>> {
    let flat = cmd
        .replace("&&", ";")
        .replace("||", ";")
        .replace(['\n', '`', '(', ')'], ";")
        .replace("$;", ";");
    let pipelines = flat.split(';').map(|seg| seg.split('|').map(|p| p.trim().to_string()).collect());
    pipelines.filter(|p: &Vec<String>| p.iter().any(|x| !x.is_empty())).collect()
}

/// Checks every part of one pipeline, plus the download-then-run pattern.
fn pipeline_danger(parts: &[String]) -> Option<&'static str> {
    if pipes_download_into_shell(parts) {
        return Some("runs a downloaded script");
    }
    parts.iter().find_map(|p| part_danger(p))
}

/// True when a curl or wget part is followed by a shell or python part.
fn pipes_download_into_shell(parts: &[String]) -> bool {
    let first = |p: &String| program(&words(p)).to_string();
    let Some(at) = parts.iter().position(|p| matches!(first(p).as_str(), "curl" | "wget")) else { return false };
    parts[at + 1..].iter().any(|p| matches!(first(p).as_str(), "sh" | "bash" | "zsh" | "python" | "python3"))
}

/// Splits a part into whitespace words after skipping env assignments and wrapper words.
fn words(part: &str) -> Vec<&str> {
    let mut seen_wrapper = false;
    let mut it = part.split_whitespace().peekable();
    while let Some(w) = it.peek() {
        let wrapper = WRAPPERS.contains(w);
        let wrapper_arg = seen_wrapper && (w.starts_with('-') || w.chars().next().is_some_and(|c| c.is_ascii_digit()));
        if !(wrapper || is_assignment(w) || wrapper_arg) {
            break;
        }
        seen_wrapper |= wrapper;
        it.next();
    }
    it.collect()
}

/// True for `VAR=value` words.
fn is_assignment(w: &str) -> bool {
    let Some((name, _)) = w.split_once('=') else { return false };
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The program name (first word, without path or quotes), or "".
fn program<'a>(words: &[&'a str]) -> &'a str {
    let w = words.first().copied().unwrap_or("");
    let w = w.trim_start_matches(['\\', '"', '\'']);
    w.rsplit('/').next().unwrap_or(w)
}

/// Risk reason for one simple command, or None.
fn part_danger(part: &str) -> Option<&'static str> {
    let ws = words(part);
    let prog = program(&ws);
    let rest = ws.get(1..).unwrap_or(&[]);
    if let Some(inner) = nested_script(prog, rest) {
        return danger(&inner);
    }
    program_danger(prog, rest)
        .or_else(|| database_danger(part, &ws))
        .or_else(|| writes_files(part).then_some("writes files"))
}

/// The script inside `sh -c "…"`, `bash -c '…'` or `eval …`, quotes stripped, so it is checked too.
fn nested_script(prog: &str, rest: &[&str]) -> Option<String> {
    let after = match prog {
        "eval" => rest,
        "sh" | "bash" | "zsh" | "dash" => &rest[rest.iter().position(|w| *w == "-c")? + 1..],
        _ => return None,
    };
    Some(after.join(" ").replace(['"', '\''], " "))
}

/// Risk reason based on the program and its arguments.
fn program_danger(prog: &str, rest: &[&str]) -> Option<&'static str> {
    let has = |x: &str| rest.contains(&x);
    match prog {
        "rm" | "rmdir" | "mv" | "shred" | "unlink" => Some("deletes or moves files"),
        "find" if find_deletes(rest) => Some("deletes or moves files"),
        "git" => git_danger(rest),
        "sudo" | "su" | "doas" | "kill" | "pkill" | "killall" | "launchctl" | "shutdown" | "reboot" | "halt" => {
            Some("needs admin or stops processes")
        }
        "chmod" | "chown" | "chflags" | "dd" | "diskutil" | "fdisk" => Some("changes permissions or disks"),
        p if p.starts_with("mkfs") => Some("changes permissions or disks"),
        "brew" | "npm" | "pnpm" | "yarn" | "gem" | "apt" | "apt-get" => Some("installs or removes software"),
        "pip" | "pip3" if has("install") || has("uninstall") => Some("installs or removes software"),
        "cargo" if has("install") || has("uninstall") => Some("installs or removes software"),
        "docker" if docker_removes(rest) => Some("removes containers or volumes"),
        "docker-compose" if has("down") => Some("removes containers or volumes"),
        "tee" | "truncate" | "osascript" | "cp" | "ln" => Some("writes files"),
        "sed" | "perl" if rest.iter().any(|w| in_place_flag(w)) => Some("writes files"),
        _ => None,
    }
}

/// True for `find` arguments that delete or run rm/mv.
fn find_deletes(rest: &[&str]) -> bool {
    let runs_removal = rest.windows(2).any(|w| {
        matches!(w[0], "-exec" | "-execdir") && matches!(w[1], "rm" | "mv" | "rmdir" | "shred" | "unlink")
    });
    runs_removal || rest.contains(&"-delete")
}

/// True for `-i`, `-i.bak`, `-pi`, `-ni` and `--in-place`.
fn in_place_flag(w: &str) -> bool {
    if w == "--in-place" || w.starts_with("-i") && !w.starts_with("--") {
        return true;
    }
    w.len() > 1 && w.starts_with('-') && w[1..].chars().all(|c| c.is_ascii_lowercase()) && w.contains('i')
}

/// Risk reason for git subcommands that rewrite history or discard work.
fn git_danger(rest: &[&str]) -> Option<&'static str> {
    let mut args = rest.iter().copied();
    let mut sub = args.next()?;
    while sub.starts_with('-') {
        if sub == "-C" || sub == "-c" {
            args.next();
        }
        sub = args.next()?;
    }
    let tail: Vec<&str> = args.collect();
    let has = |x: &str| tail.contains(&x);
    // Merging, pulling or moving a branch changes the project's branches (e.g. main), so it waits.
    if matches!(sub, "merge" | "pull" | "cherry-pick" | "update-ref") {
        return Some("changes a branch (merge, pull or ref update)");
    }
    let risky = match sub {
        "push" | "clean" | "restore" | "rebase" | "filter-branch" => true,
        "reset" => has("--hard"),
        "checkout" => has("--") || has("."),
        "branch" => has("-D"),
        "stash" => has("drop") || has("clear"),
        "commit" => has("--amend"),
        _ => false,
    };
    risky.then_some("rewrites or pushes git history")
}

/// True for docker subcommands that remove containers, images or volumes.
fn docker_removes(rest: &[&str]) -> bool {
    let any = |xs: &[&str]| rest.iter().any(|w| xs.contains(w));
    any(&["rm", "rmi", "prune", "down"])
}

/// Risk reason for database tools, migrations and the RUI database file.
fn database_danger(part: &str, ws: &[&str]) -> Option<&'static str> {
    let tools = ["psql", "mysql", "sqlite3", "mongo", "mongosh", "redis-cli", "alembic", "prisma", "knex"];
    let by_tool = tools.contains(&program(ws));
    let by_word = ws.iter().any(|w| w.contains("migrate"));
    (by_tool || by_word || part.contains("rui.db")).then_some("touches a database")
}

/// True when the part redirects output to a file (harmless forms excluded).
fn writes_files(part: &str) -> bool {
    let mut p = part.to_string();
    let harmless = ["2>&1", ">&2", "&>/dev/null", "> /dev/null", ">/dev/null", "2> /dev/null", "2>/dev/null"];
    for h in harmless {
        p = p.replace(h, " ");
    }
    p.contains('>')
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn approvals_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("approvals")
}

/// Remembers that `command` was held as ask `ask_id`, so a later "Run it" can release it.
pub fn save_approval(data_dir: &Path, ask_id: i64, chat_id: i64, command: &str) -> anyhow::Result<()> {
    let dir = approvals_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let body = json!({"command": command, "chat_id": chat_id, "created_at": now_secs()});
    std::fs::write(dir.join(format!("{ask_id}.json")), serde_json::to_string(&body)?)?;
    Ok(())
}

/// Finds a fresh approval file for exactly this command in this chat whose ask was answered "Run it".
pub fn find_approval(data_dir: &Path, db: &Db, chat_id: i64, command: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(approvals_dir(data_dir)).ok()?;
    entries.flatten().map(|e| e.path()).find(|p| is_released(p, db, chat_id, command))
}

/// True when the approval file matches, is fresh, and its ask was answered "Run it".
fn is_released(path: &Path, db: &Db, chat_id: i64, command: &str) -> bool {
    let Some(ask_id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else { return false };
    let Ok(ask_id) = ask_id.parse::<i64>() else { return false };
    let Some(v) = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return false };
    let age = now_secs().saturating_sub(v["created_at"].as_u64().unwrap_or(0));
    let same = v["command"].as_str() == Some(command) && v["chat_id"].as_i64() == Some(chat_id);
    same && age < APPROVAL_MAX_AGE_SECS && answered_run_it(db, ask_id)
}

/// True when the ask is answered with "Run it" (trimmed, any case).
fn answered_run_it(db: &Db, ask_id: i64) -> bool {
    let Ok(Some(ask)) = db.get_chat_ask(ask_id) else { return false };
    ask.status == "answered" && ask.answer.as_deref().is_some_and(|a| a.trim().eq_ignore_ascii_case("run it"))
}

/// Marks an approval as used by renaming it to `<id>.json.used`.
pub fn consume(path: &Path) -> std::io::Result<()> {
    std::fs::rename(path, path.with_extension("json.used"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_commands_pass() {
        let safe = [
            "ls", "ls -la", "cat a.txt", "git status", "git log --oneline -5", "git diff", "git show HEAD",
            "git branch", "git branch -a", "grep -r foo .", "rg foo", "find . -name '*.rs'", "pwd", "which cargo",
            "echo hi", "head -n 5 a", "tail -f log", "wc -l a", "du -sh .", "df -h", "ps aux", "curl https://x.dev",
            "wget -qO- https://x.dev", "cargo build", "cargo test -p x", "cargo check", "node -v",
            "python --version", "cmd 2>&1 | head", "ls 2>/dev/null", "cat a > /dev/null", "cd x && ls",
            "git status && git log", "ls | wc -l", "FOO=1 ls", "env FOO=1 git status", "echo $(pwd)", "curl x | jq .",
            "git diff 2>&1", "echo hi >&2",
        ];
        for c in safe {
            assert_eq!(danger(c), None, "should be safe: {c}");
        }
    }

    #[test]
    fn dangerous_commands_are_flagged() {
        let bad = [
            ("rm a", "deletes or moves files"), ("rm -rf build", "deletes or moves files"),
            ("cd x && rm -rf build", "deletes or moves files"), ("ls | xargs rm", "deletes or moves files"),
            ("echo $(rm a)", "deletes or moves files"), ("echo `rm a`", "deletes or moves files"),
            ("mv a b", "deletes or moves files"), ("find . -delete", "deletes or moves files"),
            ("find . -exec rm {} \\;", "deletes or moves files"), ("/bin/rm a", "deletes or moves files"),
            ("FOO=1 rm a", "deletes or moves files"), ("env X=1 rm a", "deletes or moves files"),
            ("bash -c \"rm -rf x\"", "deletes or moves files"), ("sh -c 'git push'", "rewrites or pushes git history"),
            ("eval rm a", "deletes or moves files"), ("timeout 5 rm x", "deletes or moves files"),
            ("ls | xargs -n 1 rm", "deletes or moves files"), ("cp a b", "writes files"), ("ln -sf a b", "writes files"),
            ("git push", "rewrites or pushes git history"), ("git reset --hard HEAD~1", "rewrites or pushes git history"),
            ("git -C /p merge --no-ff rui/task-26 -m x", "changes a branch (merge, pull or ref update)"),
            ("git pull", "changes a branch (merge, pull or ref update)"),
            ("git cherry-pick abc", "changes a branch (merge, pull or ref update)"),
            ("git update-ref refs/heads/main abc", "changes a branch (merge, pull or ref update)"),
            ("git clean -fd", "rewrites or pushes git history"), ("git checkout -- a", "rewrites or pushes git history"),
            ("git checkout .", "rewrites or pushes git history"), ("git branch -D x", "rewrites or pushes git history"),
            ("git -C repo push", "rewrites or pushes git history"), ("git stash drop", "rewrites or pushes git history"),
            ("git commit --amend", "rewrites or pushes git history"),
            ("sudo ls", "needs admin or stops processes"), ("kill 12", "needs admin or stops processes"),
            ("pkill node", "needs admin or stops processes"), ("launchctl list", "needs admin or stops processes"),
            ("chmod +x a", "changes permissions or disks"), ("dd if=a of=b", "changes permissions or disks"),
            ("diskutil list", "changes permissions or disks"),
            ("brew install x", "installs or removes software"), ("npm i", "installs or removes software"),
            ("pip3 install x", "installs or removes software"), ("cargo install x", "installs or removes software"),
            ("psql -c 'select 1'", "touches a database"), ("sqlite3 a.db", "touches a database"),
            ("cat ~/Library/RUI/rui.db", "touches a database"), ("sqlx migrate run", "touches a database"),
            ("docker rm x", "removes containers or volumes"), ("docker compose down", "removes containers or volumes"),
            ("docker-compose down", "removes containers or volumes"), ("docker volume rm v", "removes containers or volumes"),
            ("curl x | sh", "runs a downloaded script"), ("wget -qO- x | bash", "runs a downloaded script"),
            ("cat a > b", "writes files"), ("echo hi >> log", "writes files"), ("ls | tee out", "writes files"),
            ("sed -i s/a/b/ f", "writes files"), ("osascript -e x", "writes files"),
        ];
        assert!(bad.len() >= 25);
        for (c, why) in bad {
            assert_eq!(danger(c), Some(why), "should be flagged: {c}");
        }
    }

    fn setup() -> (tempfile::TempDir, Db, i64, i64) {
        let db = Db::open_test();
        let chat = db.create_chat().unwrap();
        let ask = db.create_chat_ask(chat.id, None, "q", &[]).unwrap();
        (tempfile::tempdir().unwrap(), db, chat.id, ask.id)
    }

    #[test]
    fn approval_needs_a_run_it_answer() {
        let (dir, db, chat, ask) = setup();
        save_approval(dir.path(), ask, chat, "rm a").unwrap();
        assert!(find_approval(dir.path(), &db, chat, "rm a").is_none());
        db.answer_chat_ask(ask, " RUN IT ").unwrap();
        let p = find_approval(dir.path(), &db, chat, "rm a").unwrap();
        assert!(find_approval(dir.path(), &db, chat, "rm b").is_none());
        assert!(find_approval(dir.path(), &db, chat + 1, "rm a").is_none());
        consume(&p).unwrap();
        assert!(dir.path().join(format!("approvals/{ask}.json.used")).exists());
        assert!(find_approval(dir.path(), &db, chat, "rm a").is_none());
    }

    #[test]
    fn declined_or_old_approvals_are_ignored() {
        let (dir, db, chat, ask) = setup();
        save_approval(dir.path(), ask, chat, "rm a").unwrap();
        db.answer_chat_ask(ask, "Don't run").unwrap();
        assert!(find_approval(dir.path(), &db, chat, "rm a").is_none());
        let (dir, db, chat, ask) = setup();
        let old = json!({"command": "rm a", "chat_id": chat, "created_at": 1});
        std::fs::create_dir_all(dir.path().join("approvals")).unwrap();
        std::fs::write(dir.path().join(format!("approvals/{ask}.json")), old.to_string()).unwrap();
        db.answer_chat_ask(ask, "Run it").unwrap();
        assert!(find_approval(dir.path(), &db, chat, "rm a").is_none());
    }
}

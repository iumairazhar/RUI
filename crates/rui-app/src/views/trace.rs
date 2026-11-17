//! Turns raw agent events into a readable trace, and parses unified diffs. No UI here.

use super::chat::hhmm;
use rui_core::Event;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

/// What kind of step a trace row shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Read,
    Thought,
    Changed,
    Ran,
    Stopped,
    YouSaid,
    Finished,
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Read => "Read",
            Kind::Thought => "Thought",
            Kind::Changed => "Changed",
            Kind::Ran => "Ran",
            Kind::Stopped => "Stopped",
            Kind::YouSaid => "You said",
            Kind::Finished => "Finished",
        }
    }
}

/// One step of an agent's trace.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceRow {
    pub kind: Kind,
    pub time: String,
    pub code: Option<String>,
    pub text: String,
    pub error: bool,
    pub detail: Option<Detail>,
}

/// What a row shows when expanded.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    Output { stdout: String, stderr: String },
    Patch(Vec<DiffLine>),
    Preview(String),
}

/// The result paired with a tool_use id.
struct Paired {
    text: String,
    is_error: bool,
    extra: Value,
}

fn blocks(payload: &Value) -> &[Value] {
    payload["message"]["content"].as_array().map_or(&[], |a| a.as_slice())
}

fn block_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn pair_results(events: &[Event]) -> HashMap<String, Paired> {
    let mut map = HashMap::new();
    for e in events.iter().filter(|e| e.kind == "toolresult") {
        for b in blocks(&e.payload).iter().filter(|b| b["type"] == "tool_result") {
            let Some(id) = b["tool_use_id"].as_str() else { continue };
            let paired = Paired {
                text: block_text(&b["content"]),
                is_error: b["is_error"] == true,
                extra: e.payload["tool_use_result"].clone(),
            };
            map.insert(id.to_string(), paired);
        }
    }
    map
}

/// Strip everything up to and including "/worktrees/<n>/".
pub fn rel_path(p: &str) -> String {
    let Some(i) = p.find("/worktrees/") else { return p.to_string() };
    let rest = &p[i + "/worktrees/".len()..];
    rest.split_once('/').map_or(rest.to_string(), |(_, r)| r.to_string())
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn first_str<'a>(input: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().find_map(|k| input[*k].as_str()).unwrap_or("")
}

fn row(kind: Kind, time: &str, code: Option<String>, text: impl Into<String>) -> TraceRow {
    TraceRow { kind, time: time.to_string(), code, text: text.into(), error: false, detail: None }
}

fn errored(mut r: TraceRow) -> TraceRow {
    r.error = true;
    r
}

const MAX_OUTPUT_LINES: usize = 40;
const MAX_PREVIEW_LINES: usize = 60;

/// First `max` lines, with a "… N more lines" tail when cut.
fn cap_lines(s: &str, max: usize) -> String {
    let total = s.lines().count();
    let mut out = s.lines().take(max).collect::<Vec<_>>().join("\n");
    if total > max {
        out.push_str(&format!("\n\u{2026} {} more lines", total - max));
    }
    out
}

/// Strip a leading `cd <worktree path> &&`.
fn strip_cd(cmd: &str) -> &str {
    let Some(rest) = cmd.strip_prefix("cd ") else { return cmd };
    let rest = rest.trim_start();
    let end = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => rest[1..].find(q).map(|i| i + 2),
        _ => Some(rest.find(char::is_whitespace).unwrap_or(rest.len())),
    };
    let Some(end) = end else { return cmd };
    match rest[end..].trim_start().strip_prefix("&&") {
        Some(after) if rest[..end].contains("/worktrees/") => after.trim_start(),
        _ => cmd,
    }
}

/// Where the absolute path that contains `idx` begins.
fn path_start(s: &str, idx: usize) -> usize {
    let head = &s[..idx];
    if let Some(q) = head.rfind(['"', '\'']).filter(|&q| head[q + 1..].starts_with('/')) {
        return q + 1;
    }
    head.char_indices().rev().find(|(_, c)| c.is_whitespace() || *c == '=').map_or(0, |(i, c)| i + c.len_utf8())
}

/// Remove absolute prefixes up to "/worktrees/<n>/"; a bare "…/worktrees/<n>" becomes ".".
fn strip_worktree_paths(cmd: &str) -> String {
    let (mut out, mut from) = (cmd.to_string(), 0);
    while let Some(rel) = out[from..].find("/worktrees/") {
        let after = from + rel + "/worktrees/".len();
        let digits = out[after..].chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            from = after;
            continue;
        }
        let start = path_start(&out, from + rel);
        let end = after + digits;
        let (end, rep) = if out[end..].starts_with('/') { (end + 1, "") } else { (end, ".") };
        out.replace_range(start..end, rep);
        from = start;
    }
    out
}

/// A shell command made readable: no worktree `cd`, no absolute worktree paths, one line.
pub fn clean_command(cmd: &str) -> String {
    let cmd = strip_cd(cmd.trim());
    strip_worktree_paths(cmd).lines().next().unwrap_or("").trim().to_string()
}

fn output_detail(res: &Paired) -> Option<Detail> {
    let text = |k: &str| res.extra[k].as_str().map(str::to_string);
    let stdout = text("stdout").unwrap_or_else(|| res.text.clone());
    let stderr = text("stderr").unwrap_or_default();
    if stdout.trim().is_empty() && stderr.trim().is_empty() {
        return None;
    }
    Some(Detail::Output { stdout: cap_lines(&stdout, MAX_OUTPUT_LINES), stderr: cap_lines(&stderr, MAX_OUTPUT_LINES) })
}

fn hunk_lines(h: &Value, out: &mut Vec<DiffLine>) {
    let n = |k: &str| h[k].as_u64().unwrap_or(0) as u32;
    let (mut old, mut new) = (n("oldStart"), n("newStart"));
    let head = format!("@@ -{},{} +{},{} @@", old, n("oldLines"), new, n("newLines"));
    out.push(line(DiffKind::Hunk, None, None, &head));
    for l in h["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if let Some(t) = l.strip_prefix('+') {
            out.push(line(DiffKind::Add, None, Some(new), t));
            new += 1;
        } else if let Some(t) = l.strip_prefix('-') {
            out.push(line(DiffKind::Del, Some(old), None, t));
            old += 1;
        } else {
            out.push(line(DiffKind::Ctx, Some(old), Some(new), l.strip_prefix(' ').unwrap_or(l)));
            (old, new) = (old + 1, new + 1);
        }
    }
}

fn patch_detail(input: &Value, extra: Option<&Value>) -> Option<Detail> {
    let hunks = extra.and_then(|e| e["structuredPatch"].as_array()).filter(|p| !p.is_empty());
    let mut lines = Vec::new();
    if let Some(hunks) = hunks {
        hunks.iter().for_each(|h| hunk_lines(h, &mut lines));
    } else {
        let content = input["content"].as_str().filter(|c| !c.is_empty())?;
        let adds = content.lines().enumerate().map(|(i, l)| line(DiffKind::Add, None, Some(i as u32 + 1), l));
        lines.extend(adds.take(MAX_OUTPUT_LINES));
    }
    lines.truncate(MAX_OUTPUT_LINES);
    Some(Detail::Patch(lines))
}

fn preview_detail(res: &Paired) -> Option<Detail> {
    (!res.text.trim().is_empty()).then(|| Detail::Preview(cap_lines(&res.text, MAX_PREVIEW_LINES)))
}

fn patch_counts(extra: &Value) -> Option<(usize, usize)> {
    let patches = extra["structuredPatch"].as_array().filter(|p| !p.is_empty())?;
    let lines = patches.iter().filter_map(|p| p["lines"].as_array()).flatten();
    let starts = |c: char| lines.clone().filter(|l| l.as_str().is_some_and(|s| s.starts_with(c))).count();
    Some((starts('+'), starts('-')))
}

fn changed_row(time: &str, input: &Value, res: Option<&Paired>) -> TraceRow {
    let path = rel_path(first_str(input, &["file_path", "notebook_path", "path"]));
    let counts = res.and_then(|r| patch_counts(&r.extra));
    let new_file = input["content"].as_str().map(|c| format!("New file, +{}", c.lines().count()));
    let text = match counts {
        Some((a, d)) => format!("+{a} \u{2212}{d}"),
        None => new_file.unwrap_or_default(),
    };
    let detail = patch_detail(input, res.map(|r| &r.extra));
    TraceRow { detail, ..row(Kind::Changed, time, Some(path), text) }
}

fn last_line(s: &str) -> String {
    s.lines().rev().map(str::trim).find(|l| !l.is_empty()).map_or(String::new(), |l| clip(l, 120))
}

fn bash_summary(res: &Paired) -> (String, bool) {
    let lower = res.text.to_lowercase();
    if lower.contains("requires approval") {
        return ("Blocked: needs approval".into(), true);
    }
    let out = res.extra["stdout"].as_str().unwrap_or(&res.text);
    if res.is_error {
        let err = res.extra["stderr"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(&res.text);
        return (format!("Failed \u{2014} {}", last_line(err)), true);
    }
    let test = out.lines().find_map(|l| l.split_once("test result:").map(|(_, r)| r.trim()));
    (test.map_or_else(|| last_line(out), |t| clip(t, 120)), false)
}

fn ran_row(time: &str, input: &Value, res: Option<&Paired>) -> TraceRow {
    let cmd = clip(&clean_command(input["command"].as_str().unwrap_or("")), 120);
    let (text, error) = res.map_or((String::new(), false), bash_summary);
    let detail = res.and_then(output_detail);
    TraceRow { error, detail, ..row(Kind::Ran, time, Some(cmd), text) }
}

fn mcp_row(short: &str, time: &str, input: &Value) -> Option<TraceRow> {
    let s = |k: &str| input[k].as_str().unwrap_or("");
    Some(match short {
        "report_progress" => row(Kind::Thought, time, None, s("note")),
        "ask_question" => row(Kind::Stopped, time, None, format!("Asked you: {}", s("text"))),
        "report_stuck" => errored(row(Kind::Stopped, time, None, format!("Stuck: {}", s("reason")))),
        "create_subtask" => row(Kind::Finished, time, None, format!("Created task: {}", s("title"))),
        "mark_split" => row(Kind::Finished, time, None, "Split into tasks"),
        _ => return None,
    })
}

fn tool_row(block: &Value, time: &str, results: &HashMap<String, Paired>) -> Option<TraceRow> {
    let name = block["name"].as_str().unwrap_or("");
    let input = &block["input"];
    let res = block["id"].as_str().and_then(|id| results.get(id));
    match name {
        "Read" | "Glob" | "Grep" | "LS" => {
            let code = rel_path(first_str(input, &["file_path", "pattern", "path"]));
            let detail = res.filter(|_| name == "Read").and_then(preview_detail);
            Some(TraceRow { detail, ..row(Kind::Read, time, Some(code), "") })
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Some(changed_row(time, input, res)),
        "Bash" => Some(ran_row(time, input, res)),
        _ if name.starts_with("mcp__") => mcp_row(name.rsplit("__").next()?, time, input),
        _ => None,
    }
}

fn assistant_rows(e: &Event, results: &HashMap<String, Paired>) -> Vec<TraceRow> {
    let time = hhmm(&e.created_at);
    let mut rows = Vec::new();
    for b in blocks(&e.payload) {
        match b["type"].as_str() {
            Some("tool_use") => rows.extend(tool_row(b, &time, results)),
            Some("text") => {
                let t = b["text"].as_str().unwrap_or("").trim();
                if !t.is_empty() {
                    rows.push(row(Kind::Thought, &time, None, t));
                }
            }
            _ => {}
        }
    }
    rows
}

fn done_check_row(time: &str, p: &Value) -> TraceRow {
    let passed = p["passed"] == true;
    let text = if passed {
        "Checks passed".to_string()
    } else {
        format!("Checks failed \u{2014} {}", last_line(p["output_tail"].as_str().unwrap_or("")))
    };
    TraceRow { error: !passed, ..row(Kind::Ran, time, Some("done check".into()), text) }
}

fn other_row(e: &Event) -> Option<TraceRow> {
    let (p, time) = (&e.payload, hhmm(&e.created_at));
    let text = |k: &str| p[k].as_str().unwrap_or("").to_string();
    match e.kind.as_str() {
        "instruction" => Some(row(Kind::YouSaid, &time, None, text("text"))),
        "stopped" => Some(row(Kind::Stopped, &time, None, "Stopped by you")),
        "done_check" => Some(done_check_row(&time, p)),
        "commit" if p["committed"] == true => Some(row(Kind::Finished, &time, None, "Committed")),
        "agent_error" | "scheduler_error" => Some(errored(row(Kind::Stopped, &time, None, text("error")))),
        "result" if p["is_error"] == true => {
            let t = Some(text("result")).filter(|t| !t.is_empty()).unwrap_or("Run failed".into());
            Some(errored(row(Kind::Stopped, &time, None, t)))
        }
        _ => None,
    }
}

/// Readable trace of an agent's events, oldest first.
pub fn trace(events: &[Event]) -> Vec<TraceRow> {
    let results = pair_results(events);
    let mut rows = Vec::new();
    for e in events {
        if e.kind == "assistant" {
            rows.extend(assistant_rows(e, &results));
        } else {
            rows.extend(other_row(e));
        }
    }
    rows
}

/// Unique files read or changed, changed ones first, each group sorted.
pub fn touched_files(rows: &[TraceRow], changed: &[String]) -> Vec<(String, bool)> {
    let mut hit: BTreeSet<String> = changed.iter().cloned().collect();
    let mut read = BTreeSet::new();
    for r in rows {
        let Some(code) = r.code.clone().filter(|c| !c.is_empty()) else { continue };
        match r.kind {
            Kind::Changed => hit.insert(code),
            Kind::Read => read.insert(code),
            _ => continue,
        };
    }
    let rest = read.into_iter().filter(|p| !hit.contains(p));
    let rest: Vec<String> = rest.collect();
    hit.into_iter().map(|p| (p, true)).chain(rest.into_iter().map(|p| (p, false))).collect()
}

/// Kind of one diff line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffKind {
    Hunk,
    Add,
    Del,
    Ctx,
}

/// One displayable diff line with its line numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

fn hunk_start(header: &str, sign: char) -> u32 {
    let part = header.split_whitespace().find(|p| p.starts_with(sign)).unwrap_or("");
    let num = part[1..].split(',').next().unwrap_or("");
    num.parse().unwrap_or(1)
}

fn line(kind: DiffKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine { kind, old, new, text: text.to_string() }
}

/// Parse unified diff text, skipping file header lines.
pub fn parse_diff(diff: &str) -> Vec<DiffLine> {
    let (mut out, mut in_hunk, mut old, mut new) = (Vec::new(), false, 0u32, 0u32);
    for l in diff.lines() {
        if l.starts_with("@@") {
            (old, new, in_hunk) = (hunk_start(l, '-'), hunk_start(l, '+'), true);
            out.push(line(DiffKind::Hunk, None, None, l));
        } else if l.starts_with("diff --git") {
            in_hunk = false;
        } else if !in_hunk || l.starts_with('\\') {
            continue;
        } else if let Some(t) = l.strip_prefix('+') {
            out.push(line(DiffKind::Add, None, Some(new), t));
            new += 1;
        } else if let Some(t) = l.strip_prefix('-') {
            out.push(line(DiffKind::Del, Some(old), None, t));
            old += 1;
        } else {
            out.push(line(DiffKind::Ctx, Some(old), Some(new), l.strip_prefix(' ').unwrap_or(l)));
            (old, new) = (old + 1, new + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(kind: &str, payload: Value) -> Event {
        serde_json::from_value(json!({"id":1,"task_id":7,"agent":"builder","kind":kind,
            "payload":payload,"created_at":"2026-11-15 09:41:07"}))
        .unwrap()
    }

    fn use_ev(id: &str, name: &str, input: Value) -> Event {
        let b = json!({"type":"tool_use","id":id,"name":name,"input":input});
        ev("assistant", json!({"message":{"content":[b]}}))
    }

    fn res_ev(id: &str, content: Value, is_error: bool, extra: Value) -> Event {
        let b = json!({"type":"tool_result","tool_use_id":id,"content":content,"is_error":is_error});
        ev("toolresult", json!({"message":{"content":[b]},"tool_use_result":extra}))
    }

    const P: &str = "/Users/u/Library/Application Support/RUI/worktrees/7/src/lib.rs";

    #[test]
    fn read_uses_relative_path() {
        let rows = trace(&[use_ev("a", "Read", json!({"file_path":P})), use_ev("b", "Grep", json!({"pattern":"fn x"}))]);
        assert_eq!(rows[0].kind, Kind::Read);
        assert_eq!(rows[0].code.as_deref(), Some("src/lib.rs"));
        assert_eq!(rows[0].time, "09:41");
        assert_eq!(rows[1].code.as_deref(), Some("fn x"));
    }

    #[test]
    fn edit_counts_patch_lines() {
        let patch = json!({"filePath":P,"structuredPatch":[{"oldStart":1,"oldLines":2,"newStart":1,
            "newLines":3,"lines":["+a","+b"," c","-d"]}]});
        let rows = trace(&[use_ev("a", "Edit", json!({"file_path":P})), res_ev("a", json!("ok"), false, patch)]);
        assert_eq!(rows[0].kind, Kind::Changed);
        assert_eq!(rows[0].text, "+2 \u{2212}1");
        let bare = trace(&[use_ev("z", "Write", json!({"file_path":P}))]);
        assert_eq!(bare[0].text, "");
    }

    #[test]
    fn bash_summaries() {
        let out = json!({"stdout":"running\ntest result: ok. 5 passed; 0 failed\n","stderr":""});
        let rows = trace(&[use_ev("a", "Bash", json!({"command":"cargo test\nmore"})), res_ev("a", json!("x"), false, out)]);
        assert_eq!(rows[0].code.as_deref(), Some("cargo test"));
        assert_eq!(rows[0].text, "ok. 5 passed; 0 failed");
        let last = json!({"stdout":"one\ntwo\n\n"});
        let rows = trace(&[use_ev("b", "Bash", json!({"command":"ls"})), res_ev("b", json!("x"), false, last)]);
        assert_eq!(rows[0].text, "two");
    }

    #[test]
    fn bash_blocked() {
        let c = json!([{"type":"text","text":"This command requires approval"}]);
        let rows = trace(&[use_ev("a", "Bash", json!({"command":"rm x"})), res_ev("a", c, true, json!("err"))]);
        assert!(rows[0].error);
        assert_eq!(rows[0].text, "Blocked: needs approval");
    }

    #[test]
    fn bash_failed_and_new_file() {
        let out = json!({"stdout":"","stderr":"error: could not compile\n"});
        let rows = trace(&[use_ev("a", "Bash", json!({"command":"cargo build"})), res_ev("a", json!("x"), true, out)]);
        assert!(rows[0].error);
        assert_eq!(rows[0].text, "Failed \u{2014} error: could not compile");
        let w = trace(&[use_ev("w", "Write", json!({"file_path":P,"content":"a\nb\nc\n"}))]);
        assert_eq!(w[0].text, "New file, +3");
    }

    #[test]
    fn mcp_tools_map() {
        let rows = trace(&[
            use_ev("a", "mcp__rui__report_progress", json!({"note":"halfway"})),
            use_ev("b", "mcp__rui__ask_question", json!({"text":"why?"})),
            use_ev("c", "mcp__rui__report_stuck", json!({"reason":"loop"})),
            use_ev("d", "mcp__rui__create_subtask", json!({"title":"T"})),
            use_ev("e", "mcp__rui__mark_split", json!({})),
            use_ev("f", "ToolSearch", json!({})),
            ev("tool_call", json!({"tool":"report_progress","args":{}})),
        ]);
        let got: Vec<_> = rows.iter().map(|r| (r.kind.clone(), r.text.as_str(), r.error)).collect();
        assert_eq!(got, vec![
            (Kind::Thought, "halfway", false),
            (Kind::Stopped, "Asked you: why?", false),
            (Kind::Stopped, "Stuck: loop", true),
            (Kind::Finished, "Created task: T", false),
            (Kind::Finished, "Split into tasks", false),
        ]);
    }

    #[test]
    fn assistant_text_is_thought() {
        let p = json!({"message":{"content":[{"type":"thinking","thinking":""},
            {"type":"text","text":"  "},{"type":"text","text":" Looking now. "}]}});
        let rows = trace(&[ev("assistant", p)]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind.clone(), rows[0].text.as_str()), (Kind::Thought, "Looking now."));
    }

    #[test]
    fn event_kinds_map() {
        let rows = trace(&[
            ev("instruction", json!({"text":"go"})),
            ev("instruction_delivered", json!({})),
            ev("stopped", json!({"by":"owner"})),
            ev("done_check", json!({"passed":false,"output_tail":"a\nboom\n"})),
            ev("done_check", json!({"passed":true})),
            ev("commit", json!({"committed":true})),
            ev("commit", json!({"committed":false})),
            ev("agent_error", json!({"error":"bad"})),
            ev("result", json!({"is_error":false})),
            ev("result", json!({"is_error":true})),
            ev("progress", json!({"note":"n"})),
            ev("stuck", json!({"reason":"r"})),
            ev("init", json!({})),
        ]);
        let got: Vec<_> = rows.iter().map(|r| (r.kind.label(), r.text.as_str(), r.error)).collect();
        assert_eq!(got, vec![
            ("You said", "go", false),
            ("Stopped", "Stopped by you", false),
            ("Ran", "Checks failed \u{2014} boom", true),
            ("Ran", "Checks passed", false),
            ("Finished", "Committed", false),
            ("Stopped", "bad", true),
            ("Stopped", "Run failed", true),
        ]);
    }

    #[test]
    fn clean_command_strips_worktree_noise() {
        let q = "cd \"/Users/u/Library/Application Support/RUI/worktrees/7\" && cargo test --lib powers_";
        assert_eq!(clean_command(q), "cargo test --lib powers_");
        assert_eq!(clean_command("cd /home/u/RUI/worktrees/12 && ls -la\nmore"), "ls -la");
        assert_eq!(clean_command("cd '/a b/worktrees/3' && make"), "make");
        assert_eq!(clean_command("cargo build\nx"), "cargo build");
        assert_eq!(clean_command("cd /tmp/x && ls"), "cd /tmp/x && ls");
    }

    #[test]
    fn clean_command_shortens_inner_paths() {
        let c = "cat \"/Users/u/Library/Application Support/RUI/worktrees/7/src/lib.rs\" | wc";
        assert_eq!(clean_command(c), "cat \"src/lib.rs\" | wc");
        assert_eq!(clean_command("ls /home/u/worktrees/7/src"), "ls src");
        assert_eq!(clean_command("ls /home/u/worktrees/7"), "ls .");
    }

    #[test]
    fn bash_detail_is_output() {
        let out = json!({"stdout":"a\nb","stderr":"warn"});
        let rows = trace(&[use_ev("a", "Bash", json!({"command":"ls"})), res_ev("a", json!("x"), false, out)]);
        assert_eq!(rows[0].detail, Some(Detail::Output { stdout: "a\nb".into(), stderr: "warn".into() }));
        let rows = trace(&[use_ev("b", "Bash", json!({"command":"ls"})), res_ev("b", json!("plain"), false, json!("e"))]);
        assert_eq!(rows[0].detail, Some(Detail::Output { stdout: "plain".into(), stderr: "".into() }));
        let long = "x\n".repeat(405);
        let rows = trace(&[use_ev("c", "Bash", json!({"command":"ls"})), res_ev("c", json!(long), false, json!(null))]);
        let Some(Detail::Output { stdout, .. }) = &rows[0].detail else { panic!("no output") };
        assert!(stdout.ends_with("\u{2026} 365 more lines"));
        assert_eq!(stdout.lines().count(), 41);
    }

    #[test]
    fn edit_detail_numbers_hunk_lines() {
        let patch = json!({"structuredPatch":[{"oldStart":10,"oldLines":2,"newStart":10,
            "newLines":2,"lines":[" a","-b","+c"]}]});
        let rows = trace(&[use_ev("a", "Edit", json!({"file_path":P})), res_ev("a", json!("ok"), false, patch)]);
        let Some(Detail::Patch(l)) = &rows[0].detail else { panic!("no patch") };
        assert_eq!((l[0].kind, l[0].text.as_str()), (DiffKind::Hunk, "@@ -10,2 +10,2 @@"));
        assert_eq!((l[1].old, l[1].new), (Some(10), Some(10)));
        assert_eq!((l[2].kind, l[2].old), (DiffKind::Del, Some(11)));
        assert_eq!((l[3].kind, l[3].new), (DiffKind::Add, Some(11)));
    }

    #[test]
    fn write_detail_is_all_adds() {
        let rows = trace(&[use_ev("w", "Write", json!({"file_path":P,"content":"a\nb"}))]);
        let Some(Detail::Patch(l)) = &rows[0].detail else { panic!("no patch") };
        assert_eq!(l.len(), 2);
        assert_eq!((l[1].kind, l[1].new, l[1].text.as_str()), (DiffKind::Add, Some(2), "b"));
        assert_eq!(trace(&[use_ev("e", "Edit", json!({"file_path":P}))])[0].detail, None);
    }

    #[test]
    fn read_detail_is_first_sixty_lines() {
        let text: String = (1..=70).map(|i| format!("{i}\u{2192}line\n")).collect();
        let rows = trace(&[use_ev("a", "Read", json!({"file_path":P})), res_ev("a", json!(text), false, json!(null))]);
        let Some(Detail::Preview(p)) = &rows[0].detail else { panic!("no preview") };
        assert!(p.starts_with("1\u{2192}line"));
        assert!(p.ends_with("\u{2026} 10 more lines"));
        let grep = trace(&[use_ev("g", "Grep", json!({"pattern":"x"})), res_ev("g", json!("hit"), false, json!(null))]);
        assert_eq!(grep[0].detail, None);
        let thought = trace(&[ev("assistant", json!({"message":{"content":[{"type":"text","text":"hm"}]}}))]);
        assert_eq!(thought[0].detail, None);
    }

    fn r(kind: Kind, code: &str) -> TraceRow {
        row(kind, "", Some(code.into()), "")
    }

    #[test]
    fn touched_files_orders() {
        let rows = vec![r(Kind::Read, "b.rs"), r(Kind::Read, "a.rs"), r(Kind::Changed, "z.rs"), r(Kind::Read, "a.rs"), r(Kind::Ran, "ls")];
        let got = touched_files(&rows, &["m.rs".to_string()]);
        let want = vec![("m.rs".into(), true), ("z.rs".into(), true), ("a.rs".into(), false), ("b.rs".into(), false)];
        assert_eq!(got, want);
    }

    #[test]
    fn parse_diff_numbers_lines() {
        let d = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -3,3 +3,3 @@ fn f\n a\n-b\n+c\n+d\n e";
        let got = parse_diff(d);
        assert_eq!(got.len(), 6);
        assert_eq!(got[0].kind, DiffKind::Hunk);
        assert_eq!((got[1].old, got[1].new), (Some(3), Some(3)));
        assert_eq!((got[2].kind, got[2].old, got[2].new), (DiffKind::Del, Some(4), None));
        assert_eq!((got[3].kind, got[3].new, got[3].text.as_str()), (DiffKind::Add, Some(4), "c"));
        assert_eq!(got[4].new, Some(5));
        assert_eq!((got[5].old, got[5].new), (Some(5), Some(6)));
    }
}

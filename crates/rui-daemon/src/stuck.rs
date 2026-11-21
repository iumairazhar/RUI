//! Plain-words explanations for a blocked step: the real cause from its failed checks, what was
//! already tried, and whether it keeps failing the same way (so more retries won't help).

use crate::db::Db;
use anyhow::Result;
use rui_core::{Event, QuestionStatus, Task};

/// Error the Lead gets when it tries to raise a limit on a step that keeps failing the same way.
pub const SAME_FAILURE: &str =
    "Same failure twice \u{2014} retries won't fix it. Call ask_scout or escalate to the owner with the cause.";
/// Owner options for a step stuck on its loop limit; answer_stuck_question maps them.
pub const LOOP_OPTIONS: [&str; 3] = ["Retry with more tries", "Give new instructions", "Drop this step"];
/// Owner options for a task stopped by an error.
pub const ERROR_OPTIONS: [&str; 3] = ["Retry", "Give new instructions", "Cancel"];
const CAUSE_MAX: usize = 200;
const ERROR_WORDS: [&str; 5] = ["error", "failed", "eperm", "not found", "fatal"];

/// What the owner reads in a stuck question: a headline, details and the answer buttons.
pub struct Note {
    pub headline: String,
    pub details: Vec<String>,
    pub can_do: String,
    pub options: Vec<String>,
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Blank lines and decoration (only punctuation, box drawing or a bare prompt) carry no cause.
fn is_noise(line: &str) -> bool {
    line.is_empty() || !line.chars().any(char::is_alphanumeric)
}

fn has_error_word(line: &str) -> bool {
    let low = line.to_lowercase();
    ERROR_WORDS.iter().any(|w| low.contains(w))
}

/// The first meaningful line of a check's output: one naming an error if there is one, else the first non-noise line.
pub fn cause_line(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|l| !is_noise(l)).collect();
    let pick = lines.iter().find(|l| has_error_word(l)).or(lines.first())?;
    Some(clip(pick, CAUSE_MAX))
}

/// The cause with digits, timestamps and paths taken out, so two runs of the same failure compare equal.
pub fn signature(cause: &str) -> String {
    let words = cause.split_whitespace().filter(|w| !w.contains('/') && !w.contains('\\'));
    let plain: Vec<String> = words.map(|w| w.chars().filter(|c| !c.is_ascii_digit()).collect::<String>()).collect();
    let kept: Vec<String> = plain.into_iter().filter(|w| w.chars().any(char::is_alphabetic)).collect();
    kept.join(" ").to_lowercase()
}

/// The step's failed done checks, oldest first.
fn failed_checks(db: &Db, task_id: i64) -> Result<Vec<Event>> {
    let checks = db.events_for_tasks(&[task_id], &["done_check"], 0, i64::MAX)?;
    Ok(checks.into_iter().filter(|e| e.payload["passed"].as_bool() != Some(true)).collect())
}

fn check_signature(e: &Event) -> Option<String> {
    cause_line(e.payload["output_tail"].as_str().unwrap_or_default()).map(|c| signature(&c))
}

/// When the last two failed checks fail the same way: the id of the first of them.
pub fn repeated_failure(db: &Db, task_id: i64) -> Result<Option<i64>> {
    let checks = failed_checks(db, task_id)?;
    let [.., a, b] = checks.as_slice() else { return Ok(None) };
    let same = check_signature(a).is_some_and(|s| Some(s) == check_signature(b));
    Ok(same.then_some(a.id))
}

/// True when the Lead called set_limit on `sub_id` after event `since`.
fn lead_raised_since(db: &Db, parent_id: i64, sub_id: i64, since: i64) -> Result<bool> {
    let calls = db.events_for_tasks(&[parent_id], &["tool_call"], since, i64::MAX)?;
    let raise = |e: &Event| e.agent == "lead" && e.payload["tool"] == "set_limit" && e.payload["args"]["task_id"] == sub_id;
    Ok(calls.iter().any(raise))
}

/// The retry spiral: the same failure twice and the Lead already raised the limit once since the first.
pub fn spiral(db: &Db, parent_id: i64, sub_id: i64) -> Result<bool> {
    let Some(first) = repeated_failure(db, sub_id)? else { return Ok(false) };
    lead_raised_since(db, parent_id, sub_id, first)
}

/// The step's latest cause, from its newest failed check.
pub fn latest_cause(db: &Db, task_id: i64) -> Result<Option<String>> {
    let checks = failed_checks(db, task_id)?;
    Ok(checks.last().and_then(|e| cause_line(e.payload["output_tail"].as_str().unwrap_or_default())))
}

fn first_sentence(s: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let end = line.find(". ").map_or(line.len(), |i| i + 1);
    clip(&line[..end], CAUSE_MAX)
}

/// The first sentence of the Lead's newest ruling on this step's questions.
pub fn lead_diagnosis(db: &Db, task_id: i64) -> Result<Option<String>> {
    let qs = db.questions_for_task(task_id)?;
    let ruled = qs.iter().filter(|q| q.level == "lead" && q.status == QuestionStatus::Answered);
    let last = ruled.max_by_key(|q| q.id).and_then(|q| q.answer.as_deref());
    Ok(last.map(first_sentence).filter(|s| !s.is_empty()))
}

/// "Already tried: 11 of 11 attempts. The Lead's last diagnosis: …"
fn tried_line(used: i64, max: i64, diagnosis: Option<String>) -> String {
    let mut line = format!("Already tried: {used} of {max} attempts.");
    if let Some(d) = diagnosis {
        line.push_str(&format!(" The Lead's last diagnosis: {d}"));
    }
    line
}

/// The owner's note for a step that hit its loop limit.
pub fn loop_note(db: &Db, sub: &Task, used: i64, max: i64) -> Result<Note> {
    let cause = latest_cause(db, sub.id)?.unwrap_or_else(|| "its check kept failing (no error line found)".into());
    let details = vec![format!("Cause: {cause}"), tried_line(used, max, lead_diagnosis(db, sub.id)?)];
    Ok(Note {
        headline: format!("step \"{}\" is blocked", sub.title),
        details,
        can_do: "What you can do: retry with more tries (only helps if the cause is fixed), give new instructions \
                 (for example how to fix the cause), or drop this step."
            .into(),
        options: LOOP_OPTIONS.map(String::from).to_vec(),
    })
}

/// The owner's note for a task stopped by an error.
pub fn error_note(reason: &str) -> Note {
    let cause = cause_line(reason).unwrap_or_else(|| "an unknown error".into());
    Note {
        headline: cause,
        details: vec![],
        can_do: "What you can do: retry, give new instructions, or cancel.".into(),
        options: ERROR_OPTIONS.map(String::from).to_vec(),
    }
}

/// The Lead's question for a member at its loop limit, in plain words.
pub fn lead_loop_text(db: &Db, task: &Task, used: i64, reason: &str) -> Result<String> {
    let cause = cause_line(reason).unwrap_or_else(|| "its check kept failing".into());
    let tried = tried_line(used, task.max_loops, lead_diagnosis(db, task.id)?);
    Ok(format!(
        "Step \"{}\" is blocked: it used all {} of its attempts.\nCause: {cause}\n{tried}\n\
         What can be done: if the same error repeats, retries won't fix it, so diagnose it (ask_scout) or escalate \
         to the owner with the cause. Raise the limit only if real progress is being made, or narrow the criteria.",
        task.title, task.max_loops
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind, TaskStatus};
    use serde_json::json;

    const PNPM: &str = "\n\n> site@0.0.1 build /work/12/site\n----\n ERR_PNPM_EPERM  EPERM: operation not permitted, mkdir '/Users/x/Library/pnpm/store/v3'\n at foo\n";

    #[test]
    fn cause_skips_noise_and_prefers_error_lines() {
        assert_eq!(cause_line(PNPM).unwrap(), "ERR_PNPM_EPERM  EPERM: operation not permitted, mkdir '/Users/x/Library/pnpm/store/v3'");
        assert_eq!(cause_line("\n---\nall good here\n").unwrap(), "all good here");
        assert_eq!(cause_line("\n  \n===\n"), None);
    }

    #[test]
    fn signature_ignores_digits_times_and_paths() {
        let a = signature("12:01:33 Error: cannot open /tmp/run-41/a.txt (code 13)");
        let b = signature("09:15:02 Error: cannot open /tmp/run-77/a.txt (code 13)");
        assert_eq!(a, b);
        assert_ne!(a, signature("Error: test failed"));
    }

    fn setup() -> (Db, Task, Task) {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let mk = |parent: Option<i64>, title: &str| {
            db.create_task(&NewTask {
                client_id: c.id, parent_id: parent, title: title.into(), body: String::new(), status: TaskStatus::Split,
                done_command: None, max_loops: 2, role: Role::Lead, source_kind: SourceKind::User, source_ref: None,
                agent: None, pod: vec![],
            })
            .unwrap()
        };
        let top = mk(None, "Site");
        let sub = mk(Some(top.id), "Build the site");
        (db, top, sub)
    }

    fn fail(db: &Db, id: i64, tail: &str) -> i64 {
        db.add_event(Some(id), "scheduler", "done_check", &json!({"passed": false, "output_tail": tail})).unwrap()
    }

    #[test]
    fn spiral_needs_the_same_failure_and_a_raise_in_between() {
        let (db, top, sub) = setup();
        fail(&db, sub.id, "Error: EPERM at 10:01 in /a/1");
        assert!(!spiral(&db, top.id, sub.id).unwrap());
        let raise = json!({"tool": "set_limit", "args": {"task_id": sub.id, "max_loops": 9}});
        db.add_event(Some(top.id), "lead", "tool_call", &raise).unwrap();
        fail(&db, sub.id, "Error: EPERM at 10:07 in /a/2");
        assert!(spiral(&db, top.id, sub.id).unwrap());
        fail(&db, sub.id, "Error: test x failed");
        assert!(!spiral(&db, top.id, sub.id).unwrap());
    }

    #[test]
    fn same_failure_without_a_lead_raise_is_not_a_spiral() {
        let (db, top, sub) = setup();
        fail(&db, sub.id, "Error: EPERM");
        fail(&db, sub.id, "Error: EPERM");
        assert!(repeated_failure(&db, sub.id).unwrap().is_some());
        assert!(!spiral(&db, top.id, sub.id).unwrap());
    }

    #[test]
    fn loop_note_names_the_step_cause_tries_and_diagnosis() {
        let (db, _top, sub) = setup();
        fail(&db, sub.id, PNPM);
        let q = db.create_ladder_question(sub.id, Role::Builder, "lead", &[], "help").unwrap();
        db.answer_question(q.id, "The pnpm store is outside the sandbox. Reinstall.", None).unwrap();
        let note = loop_note(&db, &sub, 11, 11).unwrap();
        assert_eq!(note.headline, "step \"Build the site\" is blocked");
        assert!(note.details[0].starts_with("Cause: ERR_PNPM_EPERM"), "{}", note.details[0]);
        assert_eq!(note.details[1], "Already tried: 11 of 11 attempts. The Lead's last diagnosis: The pnpm store is outside the sandbox.");
        assert_eq!(note.options, LOOP_OPTIONS);
    }
}

//! macOS notifications for new stuck and review inbox items.

use crate::api::InboxItem;
use std::collections::HashSet;
use std::process::Command;

/// Escape `\` and `"` for an AppleScript string literal.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// AppleScript that shows a notification titled "RUI · <title>".
pub fn notify_script(title: &str, reason: &str) -> String {
    format!("display notification \"{}\" with title \"RUI \u{b7} {}\"", escape(reason), escape(title))
}

/// Show a notification; errors are ignored.
pub fn send(title: &str, reason: &str) {
    let _ = Command::new("osascript").arg("-e").arg(notify_script(title, reason)).output();
}

/// Record every item as seen; return the stuck and review ones not seen before.
pub fn fresh(seen: &mut HashSet<(String, i64)>, items: &[InboxItem]) -> Vec<InboxItem> {
    let new = items.iter().filter(|i| seen.insert((i.kind.clone(), i.id)));
    new.filter(|i| i.kind == "stuck" || i.kind == "review").cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, id: i64) -> InboxItem {
        InboxItem { kind: kind.into(), id, task_id: None, title: "t".into(), reason: "r".into(), actions: vec![], created_at: "a".into() }
    }

    #[test]
    fn script_escapes_quotes_and_backslashes() {
        let s = notify_script("a \"b\"", "c\\d");
        assert_eq!(s, "display notification \"c\\\\d\" with title \"RUI \u{b7} a \\\"b\\\"\"");
    }

    #[test]
    fn fresh_returns_only_new_stuck_and_review() {
        let mut seen = HashSet::new();
        let items = [item("stuck", 1), item("question", 2), item("review", 3)];
        assert_eq!(fresh(&mut seen, &items).len(), 2);
        assert!(fresh(&mut seen, &items).is_empty());
        assert_eq!(fresh(&mut seen, &[item("stuck", 9)]).len(), 1);
    }
}

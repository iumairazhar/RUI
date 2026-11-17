//! Pure helpers for the Threads board: stop rules and the Done cap.

use rui_core::{Task, TaskStatus};

/// Newest cards the Done band shows before "Show more".
pub const DONE_CAP: usize = 5;

/// Running or waiting: a task the owner can still stop.
pub fn can_stop(s: &TaskStatus) -> bool {
    matches!(s, TaskStatus::New | TaskStatus::Open | TaskStatus::Clarifying | TaskStatus::InProgress | TaskStatus::WaitingAnswer)
}

/// Ids the daemon stops for `id`: the task and its direct children that are not done.
pub fn stop_targets(tasks: &[Task], id: i64) -> Vec<i64> {
    let kids = tasks.iter().filter(|t| t.parent_id == Some(id) && t.status != TaskStatus::Done);
    std::iter::once(id).chain(kids.map(|t| t.id)).collect()
}

/// Keeps the first `limit` items unless `all`; also returns how many were left out.
pub fn cap_items<T>(mut items: Vec<T>, limit: usize, all: bool) -> (Vec<T>, usize) {
    if all || items.len() <= limit {
        return (items, 0);
    }
    let hidden = items.len() - limit;
    items.truncate(limit);
    (items, hidden)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: i64, parent: Option<i64>, status: &str) -> Task {
        serde_json::from_value(json!({"id":id,"client_id":1,"parent_id":parent,"title":"t","body":"",
            "status":status,"done_command":null,"max_loops":5,"loops_used":0,"role":"builder",
            "branch":null,"source_kind":"user","source_ref":null,"depends_on":[],
            "created_at":"2026-11-15 09:00:00","updated_at":"2026-11-15 09:00:00"}))
        .unwrap()
    }

    #[test]
    fn done_cap_shows_newest_five_then_more() {
        let (shown, hidden) = cap_items((1..=8).collect::<Vec<_>>(), DONE_CAP, false);
        assert_eq!((shown, hidden), (vec![1, 2, 3, 4, 5], 3));
        assert_eq!(cap_items((1..=8).collect::<Vec<_>>(), DONE_CAP, true).1, 0);
        assert_eq!(cap_items(vec![1, 2], DONE_CAP, false), (vec![1, 2], 0));
    }

    #[test]
    fn stop_rules() {
        assert!(can_stop(&TaskStatus::InProgress) && can_stop(&TaskStatus::Open));
        assert!(!can_stop(&TaskStatus::Done) && !can_stop(&TaskStatus::Stuck) && !can_stop(&TaskStatus::Split));
        let tasks = vec![task(1, None, "split"), task(2, Some(1), "done"), task(3, Some(1), "in_progress")];
        assert_eq!(stop_targets(&tasks, 1), vec![1, 3]);
        assert_eq!(stop_targets(&tasks, 3), vec![3]);
    }
}

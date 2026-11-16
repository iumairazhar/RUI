//! Pure helpers for the Threads board: grouping into teams, bands, summaries and stop rules.

use super::{Col, parse_secs};
use rui_core::{Task, TaskStatus};
use std::collections::{HashMap, HashSet};

/// Newest teams the Landed band shows before "Show more".
pub const LANDED_CAP: usize = 5;

/// A top-level task and the tasks that work under it, in id order.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub root: i64,
    pub members: Vec<i64>,
}

/// Id of the top-level ancestor; a task whose parent is missing counts as top-level.
fn root_of(by_id: &HashMap<i64, &Task>, task: &Task) -> i64 {
    let mut cur = task;
    for _ in 0..by_id.len() {
        match cur.parent_id.and_then(|p| by_id.get(&p)) {
            Some(parent) => cur = parent,
            None => break,
        }
    }
    cur.id
}

/// Groups every task under its top-level task. Roots come in id order.
pub fn group_tasks(tasks: &[Task]) -> Vec<Group> {
    let by_id: HashMap<i64, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut ordered: Vec<&Task> = tasks.iter().collect();
    ordered.sort_by_key(|t| t.id);
    let mut groups: Vec<Group> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for t in ordered.iter().filter(|t| root_of(&by_id, t) == t.id) {
        at.insert(t.id, groups.len());
        groups.push(Group { root: t.id, members: vec![] });
    }
    for t in ordered.iter().filter(|t| root_of(&by_id, t) != t.id) {
        groups[at[&root_of(&by_id, t)]].members.push(t.id);
    }
    groups
}

fn is_stopped(t: &Task, stopped: &HashSet<i64>) -> bool {
    t.status == TaskStatus::Stuck && stopped.contains(&t.id)
}

/// Band of a team and whether it is a stopped one (it then sits in the Standing band's Stopped group).
/// Split tasks are not threads, so they do not count. `None` when nothing is left to show.
pub fn group_col(tasks: &[&Task], stopped: &HashSet<i64>) -> Option<(Col, bool)> {
    let live: Vec<&Task> = tasks.iter().copied().filter(|t| t.status != TaskStatus::Split).collect();
    let any = |f: &dyn Fn(&Task) -> bool| live.iter().any(|t| f(t));
    if live.is_empty() {
        return None;
    }
    if any(&|t| matches!(t.status, TaskStatus::WaitingAnswer | TaskStatus::Stuck) && !is_stopped(t, stopped)) {
        return Some((Col::Needs, false));
    }
    if any(&|t| matches!(t.status, TaskStatus::Clarifying | TaskStatus::InProgress)) {
        return Some((Col::Doing, false));
    }
    if any(&|t| matches!(t.status, TaskStatus::New | TaskStatus::Open)) {
        return Some((Col::Standing, false));
    }
    if any(&|t| is_stopped(t, stopped)) {
        return Some((Col::Standing, true));
    }
    Some((Col::Landed, false))
}

/// A team starts expanded when something in it is running or needs the owner.
pub fn default_expanded(tasks: &[&Task], stopped: &HashSet<i64>) -> bool {
    matches!(group_col(tasks, stopped), Some((Col::Needs | Col::Doing, _)))
}

/// "2 landed, 1 in the loop": what the members are doing, in a fixed order.
pub fn team_summary(members: &[&Task], stopped: &HashSet<i64>) -> String {
    let count = |f: &dyn Fn(&Task) -> bool| members.iter().filter(|t| f(t)).count();
    let needs = |t: &Task| matches!(t.status, TaskStatus::WaitingAnswer | TaskStatus::Stuck) && !is_stopped(t, stopped);
    let parts = [
        (count(&|t| t.status == TaskStatus::Done), "landed"),
        (count(&|t| t.status == TaskStatus::InProgress), "in the loop"),
        (count(&|t| t.status == TaskStatus::Clarifying), "thinking"),
        (count(&|t| matches!(t.status, TaskStatus::New | TaskStatus::Open)), "waiting"),
        (count(&needs), "need you"),
        (count(&|t| is_stopped(t, stopped)), "stopped"),
    ];
    let parts: Vec<String> = parts.iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect();
    parts.join(", ")
}

/// The one line of a collapsed team: "Title · 3 agents · 2 landed, 1 in the loop".
pub fn collapsed_line(title: &str, agents: usize, summary: &str) -> String {
    let noun = if agents == 1 { "agent" } else { "agents" };
    let head = format!("{title} \u{b7} {agents} {noun}");
    if summary.is_empty() { head } else { format!("{head} \u{b7} {summary}") }
}

/// Running or waiting: a task the owner can still stop.
pub fn can_stop(s: &TaskStatus) -> bool {
    matches!(s, TaskStatus::New | TaskStatus::Open | TaskStatus::Clarifying | TaskStatus::InProgress | TaskStatus::WaitingAnswer)
}

/// "Stop team" on a Lead or parent row, "Stop" on a member.
pub fn stop_label(is_team: bool) -> &'static str {
    if is_team { "Stop team" } else { "Stop" }
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

/// When the team last changed, in unix seconds.
pub fn latest_stamp(tasks: &[&Task]) -> i64 {
    tasks.iter().filter_map(|t| parse_secs(&t.updated_at)).max().unwrap_or(0)
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

    fn refs(tasks: &[Task]) -> Vec<&Task> {
        tasks.iter().collect()
    }

    #[test]
    fn groups_members_under_top_level_tasks() {
        let tasks = vec![
            task(1, None, "split"),
            task(2, Some(1), "done"),
            task(3, None, "open"),
            task(4, Some(1), "in_progress"),
            task(5, Some(4), "new"),
        ];
        let groups = group_tasks(&tasks);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], Group { root: 1, members: vec![2, 4, 5] });
        assert_eq!(groups[1], Group { root: 3, members: vec![] });
    }

    #[test]
    fn orphan_counts_as_top_level() {
        let groups = group_tasks(&[task(7, Some(99), "open")]);
        assert_eq!(groups, vec![Group { root: 7, members: vec![] }]);
    }

    #[test]
    fn collapsed_summary_text() {
        let tasks = vec![task(2, Some(1), "done"), task(3, Some(1), "done"), task(4, Some(1), "in_progress")];
        let none = HashSet::new();
        let summary = team_summary(&refs(&tasks), &none);
        assert_eq!(summary, "2 landed, 1 in the loop");
        assert_eq!(collapsed_line("Returns", 3, &summary), "Returns \u{b7} 3 agents \u{b7} 2 landed, 1 in the loop");
        assert_eq!(collapsed_line("Solo", 1, ""), "Solo \u{b7} 1 agent");
        let stuck = vec![task(5, Some(1), "stuck")];
        assert_eq!(team_summary(&refs(&stuck), &HashSet::from([5])), "1 stopped");
        assert_eq!(team_summary(&refs(&stuck), &none), "1 need you");
    }

    #[test]
    fn landed_cap_shows_newest_five_then_more() {
        let (shown, hidden) = cap_items((1..=8).collect::<Vec<_>>(), LANDED_CAP, false);
        assert_eq!((shown, hidden), (vec![1, 2, 3, 4, 5], 3));
        assert_eq!(cap_items((1..=8).collect::<Vec<_>>(), LANDED_CAP, true).1, 0);
        assert_eq!(cap_items(vec![1, 2], LANDED_CAP, false), (vec![1, 2], 0));
    }

    #[test]
    fn stop_label_and_rules() {
        assert_eq!(stop_label(true), "Stop team");
        assert_eq!(stop_label(false), "Stop");
        assert!(can_stop(&TaskStatus::InProgress) && can_stop(&TaskStatus::Open));
        assert!(!can_stop(&TaskStatus::Done) && !can_stop(&TaskStatus::Stuck) && !can_stop(&TaskStatus::Split));
        let tasks = vec![task(1, None, "split"), task(2, Some(1), "done"), task(3, Some(1), "in_progress")];
        assert_eq!(stop_targets(&tasks, 1), vec![1, 3]);
        assert_eq!(stop_targets(&tasks, 3), vec![3]);
    }

    #[test]
    fn expanded_by_default_only_when_active() {
        let none = HashSet::new();
        let running = vec![task(1, None, "split"), task(2, Some(1), "in_progress"), task(3, Some(1), "done")];
        assert!(default_expanded(&refs(&running), &none));
        let needs = vec![task(2, Some(1), "waiting_answer")];
        assert!(default_expanded(&refs(&needs), &none));
        let landed = vec![task(1, None, "done"), task(2, Some(1), "done")];
        assert!(!default_expanded(&refs(&landed), &none));
        let waiting = vec![task(2, Some(1), "open")];
        assert!(!default_expanded(&refs(&waiting), &none));
        let stopped = vec![task(2, Some(1), "stuck")];
        assert!(!default_expanded(&refs(&stopped), &HashSet::from([2])));
    }

    #[test]
    fn bands_follow_member_states() {
        let none = HashSet::new();
        let mixed = vec![task(1, None, "split"), task(2, Some(1), "done"), task(3, Some(1), "in_progress")];
        assert_eq!(group_col(&refs(&mixed), &none), Some((Col::Doing, false)));
        let stopped = vec![task(2, Some(1), "done"), task(3, Some(1), "stuck")];
        assert_eq!(group_col(&refs(&stopped), &HashSet::from([3])), Some((Col::Standing, true)));
        assert_eq!(group_col(&refs(&stopped), &none), Some((Col::Needs, false)));
        assert_eq!(group_col(&refs(&[task(1, None, "split")]), &none), None);
    }
}

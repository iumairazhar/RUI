//! Role titles, member numbering, pod lines and loop counts shared by Threads, Team and Skills.

use crate::api::RoleInfo;
use rui_core::{Role, Task};

/// "backend" becomes "Backend" when the roster has no title for it.
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Display title of a role, e.g. "Backend Engineer".
pub fn role_title(roles: &[RoleInfo], agent: &str) -> String {
    roles.iter().find(|r| r.name == agent).map_or_else(|| capitalize(agent), |r| r.title.clone())
}

/// Two-letter code of a role, e.g. "BE".
pub fn role_code(roles: &[RoleInfo], agent: &str) -> String {
    let found = roles.iter().find(|r| r.name == agent).map(|r| r.code.clone());
    found.unwrap_or_else(|| agent.chars().take(2).collect::<String>().to_uppercase())
}

/// "Backend Engineer 01": the title and the member's place among siblings with the same agent.
/// Top-level tasks have no siblings, so they get the plain title. `None` without an agent.
pub fn member_name(task: &Task, tasks: &[Task], roles: &[RoleInfo]) -> Option<String> {
    let agent = task.agent.as_deref()?;
    let title = role_title(roles, agent);
    if task.parent_id.is_none() {
        return Some(title);
    }
    let earlier = |t: &&Task| t.parent_id == task.parent_id && t.agent == task.agent && t.id < task.id;
    Some(format!("{title} {:02}", tasks.iter().filter(earlier).count() + 1))
}

/// Codes of a pod's roles, without the lead.
pub fn pod_codes(pod: &[String], roles: &[RoleInfo]) -> Vec<String> {
    pod.iter().filter(|r| r.as_str() != "lead").map(|r| role_code(roles, r)).collect()
}

/// "Lead · Pod: BE · FE · TS" for a lead parent with a staffed pod.
pub fn pod_line(task: &Task, roles: &[RoleInfo]) -> Option<String> {
    let codes = pod_codes(&task.pod, roles);
    if task.role != Role::Lead || codes.is_empty() {
        return None;
    }
    Some(format!("Lead \u{b7} Pod: {}", codes.join(" \u{b7} ")))
}

/// One loop dot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dot {
    Off,
    On,
    Over,
}

/// Loops a task has used against its limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loops {
    pub used: i64,
    pub max: i64,
}

impl Loops {
    /// Past the limit.
    pub fn over(&self) -> bool {
        self.used > self.max
    }

    /// "2/5".
    pub fn text(&self) -> String {
        format!("{}/{}", self.used, self.max)
    }

    /// One dot per allowed loop, or per used loop when that is more.
    pub fn dots(&self) -> Vec<Dot> {
        let total = self.used.max(self.max).clamp(0, 30);
        let dot = |i: i64| match (i < self.used, i >= self.max) {
            (true, true) => Dot::Over,
            (true, false) => Dot::On,
            _ => Dot::Off,
        };
        (0..total).map(dot).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn role(name: &str, code: &str, title: &str, skills: &[&str]) -> RoleInfo {
        RoleInfo {
            name: name.into(),
            code: code.into(),
            title: title.into(),
            description: String::new(),
            skills: skills.iter().map(|s| s.to_string()).collect(),
            max_loops: 5,
            ..Default::default()
        }
    }

    fn task(id: i64, parent: Option<i64>, agent: Option<&str>) -> Task {
        serde_json::from_value(json!({"id":id,"client_id":1,"parent_id":parent,"title":"t","body":"",
            "status":"open","done_command":null,"max_loops":5,"loops_used":0,"role":"builder",
            "branch":null,"source_kind":"user","source_ref":null,"depends_on":[],"agent":agent,
            "created_at":"a","updated_at":"b"}))
        .unwrap()
    }

    #[test]
    fn names_number_siblings_with_the_same_agent() {
        let roles = [role("backend", "BE", "Backend Engineer", &[])];
        let tasks = vec![
            task(1, None, Some("lead")),
            task(2, Some(1), Some("backend")),
            task(3, Some(1), Some("tester")),
            task(4, Some(1), Some("backend")),
            task(9, Some(8), Some("backend")),
        ];
        let name = |i: usize| member_name(&tasks[i], &tasks, &roles);
        assert_eq!(name(1).as_deref(), Some("Backend Engineer 01"));
        assert_eq!(name(3).as_deref(), Some("Backend Engineer 02"));
        assert_eq!(name(2).as_deref(), Some("Tester 01"));
        assert_eq!(name(4).as_deref(), Some("Backend Engineer 01"));
        assert_eq!(name(0).as_deref(), Some("Lead"));
        assert_eq!(member_name(&task(5, Some(1), None), &tasks, &roles), None);
    }

    #[test]
    fn codes_fall_back_to_the_name() {
        let roles = [role("backend", "BE", "Backend Engineer", &[])];
        assert_eq!((role_code(&roles, "backend"), role_code(&roles, "tester")), ("BE".into(), "TE".into()));
        let pod: Vec<String> = ["lead", "backend", "tester"].map(String::from).into();
        assert_eq!(pod_codes(&pod, &roles), ["BE", "TE"]);
    }

    #[test]
    fn pod_line_only_for_lead_with_pod() {
        let roles = [role("backend", "BE", "Backend Engineer", &[]), role("frontend", "FE", "Frontend Engineer", &[])];
        let mut lead = task(1, None, Some("lead"));
        assert_eq!(pod_line(&lead, &roles), None);
        lead.role = Role::Lead;
        lead.pod = ["lead", "backend", "frontend"].map(String::from).into();
        assert_eq!(pod_line(&lead, &roles).as_deref(), Some("Lead \u{b7} Pod: BE \u{b7} FE"));
    }

    #[test]
    fn loop_dots_state() {
        use Dot::*;
        let l = Loops { used: 2, max: 4 };
        assert_eq!((l.dots(), l.text(), l.over()), (vec![On, On, Off, Off], "2/4".into(), false));
        let full = Loops { used: 4, max: 4 };
        assert_eq!((full.dots(), full.over()), (vec![On, On, On, On], false));
        let over = Loops { used: 6, max: 4 };
        assert_eq!(over.dots(), vec![On, On, On, On, Over, Over]);
        assert_eq!((over.text(), over.over()), ("6/4".into(), true));
        assert!(Loops { used: 0, max: 0 }.dots().is_empty());
    }
}

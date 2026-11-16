//! Team: pods in flight and the roster of roles. Minimal; a fuller Agents page replaces it later.

use super::activity::{MONO, icon, mono};
use super::character::{CharState, character};
use super::roster::{role_code, role_title};
use super::{card, page_head, role_chip, tag};
use crate::api::RoleInfo;
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, FontWeight, Window, div, prelude::*, px};
use rui_core::{Role, Task, TaskStatus};

const OWNS_CHECKS: [&str; 2] = ["tester", "evaluator"];

/// Team page.
pub struct TeamView {
    store: Entity<Store>,
}

/// Is a member of this pod with this agent running right now.
fn is_live(pod: &Task, agent: &str, tasks: &[Task], running: &dyn Fn(&Task) -> bool) -> bool {
    let mine = |t: &&Task| (t.id == pod.id || t.parent_id == Some(pod.id)) && t.agent.as_deref() == Some(agent);
    tasks.iter().filter(mine).any(running)
}

/// Lead parents that are not done.
fn pods_in_flight(tasks: &[Task]) -> Vec<&Task> {
    tasks.iter().filter(|t| t.role == Role::Lead && t.parent_id.is_none() && t.status != TaskStatus::Done).collect()
}

/// "2 pods holding 5 agents, ..." counts: (pods, agents).
fn counts(pods: &[&Task]) -> (usize, usize) {
    (pods.len(), pods.iter().map(|p| p.pod.len()).sum())
}

fn heading(text: &str, th: &Theme) -> Div {
    let h = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted);
    h.mt(px(24.)).mb(px(10.)).child(text.to_uppercase())
}

impl TeamView {
    /// Build the team view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store }
    }

    fn pod_role(&self, pod: &Task, i: usize, agent: &str, color: gpui::Hsla, cx: &Context<Self>) -> AnyElement {
        let th = theme(cx);
        let s = self.store.read(cx);
        let running = |t: &Task| s.live.task(t.id).is_some() || matches!(t.status, TaskStatus::InProgress | TaskStatus::Clarifying);
        let live = is_live(pod, agent, &s.tasks, &running);
        let state: AnyElement = match live {
            true => {
                let id = ("team-rui", pod.id as usize * 32 + i);
                let face = character(CharState::Doing, px(18.), id, color, th);
                div().flex().items_center().gap(px(5.)).child(face).child("In the loop").into_any_element()
            }
            false => div().child("Idle").into_any_element(),
        };
        let name = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(role_title(&s.roles, agent));
        let sub = div().text_size(px(10.5)).text_color(th.muted).child(state);
        let text = div().flex().flex_col().child(name).child(sub);
        let item = div().flex().items_center().gap(px(9.)).px(px(8.)).py(px(8.)).rounded(px(10.)).bg(th.well);
        item.child(role_chip(&role_code(&s.roles, agent), 28., color)).child(text).into_any_element()
    }

    fn pod_card(&self, pod: &Task, cx: &Context<Self>) -> AnyElement {
        let th = theme(cx);
        let s = self.store.read(cx);
        let project = s.clients.iter().find(|c| c.id == pod.client_id).map(|c| c.name.clone()).unwrap_or_default();
        let color = th.client_color(pod.client_id);
        let roles: Vec<AnyElement> =
            pod.pod.iter().enumerate().map(|(i, a)| self.pod_role(pod, i, a, color, cx)).collect();
        let title = div().text_size(px(14.)).font_weight(FontWeight::BOLD).child(pod.title.clone());
        let proj = div().text_size(px(11.5)).text_color(th.muted).child(project);
        let max = div().ml_auto().text_size(px(11.5)).text_color(th.muted).child(format!("max loops {}", pod.max_loops));
        let head = div().flex().items_center().gap(px(8.)).mb(px(12.)).child(title).child(proj).child(max);
        let roles = div().flex().flex_wrap().gap(px(8.)).children(roles);
        card(cx).id(("pod", pod.id as usize)).p(px(14.)).max_w(px(1040.)).child(head).child(roles).into_any_element()
    }

    fn role_row(&self, i: usize, r: &RoleInfo, cx: &Context<Self>) -> AnyElement {
        let th = theme(cx);
        let color = th.client_color(1);
        let desc = div().text_size(px(11.5)).text_color(th.muted).child(r.description.clone());
        let title = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(r.title.clone());
        let muted = super::activity::muted_status(th);
        let skills = r.skills.iter().map(|k| tag(&muted, k).font_family(MONO));
        let skills = div().flex().flex_wrap().gap(px(4.)).children(skills);
        let body = div().flex().flex_col().flex_1().min_w_0().gap(px(3.)).child(title).child(desc).child(skills);
        let lock = OWNS_CHECKS.contains(&r.name.as_str()).then(|| owns_checks(th));
        let tools = mono(11., th.muted, format!("{} of 6", r.tools.len()));
        let row = div().id(("role-row", i)).flex().items_center().gap(px(12.)).px(px(12.)).py(px(10.));
        let row = row.rounded(px(10.)).bg(th.surface).shadow(th.card_shadow());
        row.child(role_chip(&r.code, 28., color)).child(body).children(lock).child(tools).into_any_element()
    }
}

fn owns_checks(th: &Theme) -> Div {
    let lock = icon("lock", 12., th.block.fg);
    let chip = div().flex().items_center().gap(px(5.)).h(px(22.)).px(px(9.)).rounded(px(7.));
    let chip = chip.bg(th.block.dim).border_1().border_color(th.block.rim).text_color(th.block.fg);
    chip.text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).child(lock).child("Owns the checks")
}

impl Render for TeamView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let (tasks, roles) = {
            let s = self.store.read(cx);
            (s.tasks.clone(), s.roles.clone())
        };
        let pods = pods_in_flight(&tasks);
        let (n, m) = counts(&pods);
        let lead = format!("{n} pods holding {m} agents, each built from the ten roles. A pod takes only the roles its work needs.");
        let lead = div().id("team-lead").text_size(px(14.)).text_color(th.fg_2).child(lead);
        let cards: Vec<AnyElement> = pods.iter().map(|p| self.pod_card(p, cx)).collect();
        let rows: Vec<AnyElement> = roles.iter().enumerate().map(|(i, r)| self.role_row(i, r, cx)).collect();
        let note = "No agent may edit the checks that judge its own output.";
        let note = div().mt(px(10.)).text_size(px(11.5)).text_color(th.muted).child(note);
        div()
            .id("team-scroll")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .pr(px(40.))
            .pb(px(28.))
            .child(page_head("ROSTER", "Team", cx))
            .child(lead)
            .child(heading("Pods in flight", &th))
            .child(div().flex().flex_col().gap(px(8.)).children(cards))
            .child(heading("The ten roles", &th))
            .child(div().flex().flex_col().gap(px(6.)).children(rows))
            .child(note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: i64, parent: Option<i64>, agent: &str, role: &str, status: &str) -> Task {
        serde_json::from_value(json!({"id":id,"client_id":1,"parent_id":parent,"title":"t","body":"",
            "status":status,"done_command":null,"max_loops":5,"loops_used":0,"role":role,
            "branch":null,"source_kind":"user","source_ref":null,"depends_on":[],"agent":agent,
            "pod":["lead","backend"],"created_at":"a","updated_at":"b"}))
        .unwrap()
    }

    #[test]
    fn live_members_and_pods() {
        let tasks = vec![
            task(1, None, "lead", "lead", "split"),
            task(2, Some(1), "backend", "builder", "in_progress"),
            task(3, Some(1), "tester", "builder", "open"),
            task(4, None, "lead", "lead", "done"),
        ];
        let running = |t: &Task| t.status == TaskStatus::InProgress;
        assert!(is_live(&tasks[0], "backend", &tasks, &running));
        assert!(!is_live(&tasks[0], "tester", &tasks, &running));
        let pods = pods_in_flight(&tasks);
        assert_eq!(pods.len(), 1);
        assert_eq!(counts(&pods), (1, 2));
    }
}

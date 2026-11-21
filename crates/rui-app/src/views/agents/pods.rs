//! Pods tab: working teams as compact cards, finished teams as one-line rows under a fold.

use super::{AgentsView, group_pods, heading, role_word};
use crate::api::OwnerState;
use crate::theme::{Theme, theme};
use crate::views::activity::chevron;
use crate::views::character::{CharState, character};
use crate::views::roster::{role_code, role_title};
use crate::views::status::{self, Word, word_of};
use crate::views::{card, chip, role_chip};
use gpui::{AnyElement, Context, Div, FontWeight, Hsla, div, prelude::*, px};
use rui_core::{Client, Task};
use std::collections::HashMap;

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// ---- pure helpers ----

/// Every member is done, stopped or never staffed (dropped), and at least one ran.
pub(super) fn is_finished(words: &[Option<Word>]) -> bool {
    let over = |w: &Option<Word>| matches!(w, None | Some(Word::Done | Word::Stopped));
    words.iter().any(Option::is_some) && words.iter().all(over)
}

fn words_of(pod: &Task, tasks: &[Task], states: &HashMap<i64, OwnerState>) -> Vec<Option<Word>> {
    pod.pod.iter().map(|a| role_word(pod, a, tasks, states)).collect()
}

/// Pods split into (working, finished), keeping their order.
pub(super) fn split_pods<'a>(pods: &[&'a Task], tasks: &[Task], states: &HashMap<i64, OwnerState>) -> (Vec<&'a Task>, Vec<&'a Task>) {
    pods.iter().copied().partition(|p| !is_finished(&words_of(p, tasks, states)))
}

/// Newest `updated_at` of the pod and its members.
pub(super) fn last_update<'a>(pod: &'a Task, tasks: &'a [Task]) -> Option<&'a str> {
    let mine = tasks.iter().filter(|t| t.parent_id == Some(pod.id)).map(|t| t.updated_at.as_str());
    std::iter::once(pod.updated_at.as_str()).chain(mine).filter(|s| !s.is_empty()).max()
}

/// "done 15 Nov" from "2026-11-15 09:00:00"; None when the date does not parse.
pub(super) fn done_text(stamp: &str) -> Option<String> {
    let mut parts = stamp.get(..10)?.split('-');
    let (_, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let month = MONTHS.get(m.parse::<usize>().ok()?.checked_sub(1)?)?;
    Some(format!("done {} {month}", d.parse::<u32>().ok()?))
}

/// Short role codes of a pod, e.g. ["LD", "RS", "WR"].
pub(super) fn initials(pod: &[String], roles: &[crate::api::RoleInfo]) -> Vec<String> {
    pod.iter().map(|a| role_code(roles, a)).collect()
}

fn status_dot(color: Hsla) -> Div {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

fn muted(text: impl Into<gpui::SharedString>, size: f32, th: &Theme) -> Div {
    div().flex_none().text_size(px(size)).text_color(th.muted).child(text.into())
}

// ---- rendering ----

impl AgentsView {
    /// One member tile: avatar, role title and its status word.
    fn pod_role(&self, pod: &Task, i: usize, agent: &str, color: Hsla, cx: &Context<Self>) -> AnyElement {
        let th = theme(cx);
        let s = self.store.read(cx);
        let word = role_word(pod, agent, &s.tasks, &s.states);
        let fg = word.map_or(th.muted, |w| status::style(w, th).fg);
        let face = (word == Some(Word::Working)).then(|| character(CharState::Doing, px(16.), ("team-rui", pod.id as usize * 32 + i), color, th));
        let state = div().flex().items_center().gap(px(4.)).text_size(px(11.)).text_color(fg).children(face).child(word.map_or("Idle", Word::label));
        let name = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(role_title(&s.roles, agent));
        let text = div().flex().flex_col().child(name).child(state);
        let item = div().flex().items_center().gap(px(8.)).p(px(8.)).rounded(px(9.)).bg(th.well);
        item.child(role_chip(&role_code(&s.roles, agent), 28., color)).child(text).into_any_element()
    }

    fn pod_tiles(&self, pod: &Task, cx: &Context<Self>) -> Div {
        let color = theme(cx).client_color(pod.client_id);
        let tiles: Vec<AnyElement> = pod.pod.iter().enumerate().map(|(i, a)| self.pod_role(pod, i, a, color, cx)).collect();
        div().flex().flex_wrap().gap(px(8.)).children(tiles)
    }

    /// "Resume team" on a stopped pod: instructing its top task resumes the chain.
    fn resume_button(&self, pod_id: i64, cx: &mut Context<Self>) -> AnyElement {
        let b = chip(format!("pod-resume-{pod_id}"), "Resume team", cx);
        let on_click = cx.listener(move |this, _, _, cx| {
            let text = "Resume where you left off.".to_string();
            this.store.update(cx, |s, cx| s.instruct_task(pod_id, text, cx));
        });
        b.on_click(on_click).into_any_element()
    }

    /// "Title · 3 roles · 1 working" on one row with the client dot.
    fn pod_head(&self, pod: &Task, cx: &mut Context<Self>) -> Div {
        let th = theme(cx).clone();
        let s = self.store.read(cx);
        let working = words_of(pod, &s.tasks, &s.states).into_iter().filter(|w| *w == Some(Word::Working)).count();
        let stopped = word_of(&s.states, pod.id) == Word::Stopped;
        let title = div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(13.5)).font_weight(FontWeight::BOLD).child(pod.title.clone());
        let meta = muted(format!("\u{b7} {} roles \u{b7} {working} working", pod.pod.len()), 11.5, &th);
        let resume = stopped.then(|| self.resume_button(pod.id, cx));
        let row = div().flex().items_center().gap(px(8.)).mb(px(10.));
        row.child(status_dot(th.client_color(pod.client_id))).child(title).child(meta).child(div().flex_1()).children(resume)
    }

    fn pod_card(&self, pod: &Task, cx: &mut Context<Self>) -> AnyElement {
        let head = self.pod_head(pod, cx);
        let tiles = self.pod_tiles(pod, cx);
        card(cx).id(("pod", pod.id as usize)).p(px(14.)).max_w(px(1040.)).child(head).child(tiles).into_any_element()
    }

    /// Client label and cards for each group that still has a working team.
    fn working_groups(&self, clients: &[Client], working: &[&Task], th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if working.is_empty() {
            return vec![muted("No team is working right now.", 12.5, th).px(px(14.)).py(px(8.)).into_any_element()];
        }
        let mut out = Vec::new();
        for (name, pods) in group_pods(clients, working) {
            let cards: Vec<AnyElement> = pods.iter().map(|p| self.pod_card(p, cx)).collect();
            let label = heading(&name, th).mt(px(16.)).mb(px(8.)).px(px(14.));
            out.push(div().flex().flex_col().child(label).child(div().flex().flex_col().gap(px(8.)).children(cards)).into_any_element());
        }
        out
    }

    fn toggle_finished(&mut self, cx: &mut Context<Self>) {
        self.show_finished = !self.show_finished;
        cx.notify();
    }

    fn toggle_pod(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.open_pods.remove(&id) {
            self.open_pods.insert(id);
        }
        cx.notify();
    }

    /// "Finished (N)" header that folds the finished rows.
    fn finished_head(&self, n: usize, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let text = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(format!("FINISHED ({n})"));
        let row = div().id("pods-finished-toggle").flex().items_center().gap(px(6.)).mt(px(24.)).mb(px(8.)).px(px(14.)).cursor_pointer();
        row.child(chevron(self.show_finished, th)).child(text).on_click(cx.listener(|this, _, _, cx| this.toggle_finished(cx))).into_any_element()
    }

    /// One line per finished team; clicking it shows its member tiles inline.
    fn finished_row(&self, pod: &Task, clients: &[Client], th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let color = th.client_color(pod.client_id);
        let client = clients.iter().find(|c| c.id == pod.client_id).map_or(String::new(), |c| c.name.clone());
        let codes = initials(&pod.pod, &s.roles).into_iter().map(|c| role_chip(&c, 18., color));
        let when = last_update(pod, &s.tasks).and_then(done_text).map(|t| muted(t, 11., th));
        let title = div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(pod.title.clone());
        let chips = div().flex().flex_none().gap(px(3.)).children(codes);
        let line = div().flex().items_center().gap(px(8.)).child(status_dot(color)).child(title).child(muted(client, 11.5, th)).child(chips).children(when);
        let open = self.open_pods.contains(&pod.id);
        let tiles = open.then(|| self.pod_tiles(pod, cx).mt(px(10.)));
        let id = pod.id;
        let row = card(cx).id(("pod-done", pod.id as usize)).px(px(14.)).py(px(8.)).max_w(px(1040.)).cursor_pointer();
        row.child(line).children(tiles).on_click(cx.listener(move |this, _, _, cx| this.toggle_pod(id, cx))).into_any_element()
    }

    /// Working groups, then the folded Finished list.
    pub(super) fn pod_sections(&self, pods: &[&Task], clients: &[Client], th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (working, finished) = {
            let s = self.store.read(cx);
            split_pods(pods, &s.tasks, &s.states)
        };
        let mut out = self.working_groups(clients, &working, th, cx);
        if finished.is_empty() {
            return out;
        }
        out.push(self.finished_head(finished.len(), th, cx));
        if self.show_finished {
            let rows: Vec<AnyElement> = finished.iter().map(|p| self.finished_row(p, clients, th, cx)).collect();
            out.push(div().flex().flex_col().gap(px(6.)).children(rows).into_any_element());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: i64, parent: Option<i64>, agent: &str, updated: &str) -> Task {
        serde_json::from_value(json!({"id":id,"client_id":1,"parent_id":parent,"title":"t","body":"",
            "status":"split","done_command":null,"max_loops":5,"loops_used":0,"role":"lead",
            "branch":null,"source_kind":"user","source_ref":null,"depends_on":[],"agent":agent,
            "pod":["lead","writer"],"created_at":"a","updated_at":updated}))
        .unwrap()
    }

    fn state(id: i64, word: &str) -> (i64, OwnerState) {
        (id, OwnerState { task_id: id, word: word.into(), reason: String::new(), since: String::new() })
    }

    #[test]
    fn finished_needs_every_member_over() {
        assert!(is_finished(&[Some(Word::Done), Some(Word::Stopped), None]));
        assert!(!is_finished(&[Some(Word::Done), Some(Word::Working)]));
        assert!(!is_finished(&[Some(Word::Waiting)]));
        assert!(!is_finished(&[None, None]));
    }

    #[test]
    fn pods_split_into_working_and_finished() {
        let tasks = vec![task(1, None, "lead", "x"), task(2, Some(1), "writer", "x"), task(3, None, "lead", "x")];
        let states: HashMap<i64, OwnerState> = [state(1, "Done"), state(2, "Done"), state(3, "Working")].into();
        let pods: Vec<&Task> = vec![&tasks[0], &tasks[2]];
        let (working, finished) = split_pods(&pods, &tasks, &states);
        assert_eq!((working[0].id, finished[0].id), (3, 1));
    }

    #[test]
    fn done_date_uses_newest_member() {
        let tasks = vec![task(1, None, "lead", "2026-11-15 09:00:00"), task(2, Some(1), "writer", "2026-11-17 10:00:00")];
        assert_eq!(last_update(&tasks[0], &tasks).and_then(done_text).as_deref(), Some("done 17 Nov"));
        assert_eq!(done_text("2026-01-05T03:00:00Z").as_deref(), Some("done 5 Jan"));
        assert_eq!(done_text("b"), None);
        assert_eq!(done_text("2026-13-01 00:00:00"), None);
    }

    #[test]
    fn initials_use_role_codes() {
        let roles = vec![crate::api::RoleInfo { name: "lead".into(), code: "LD".into(), ..Default::default() }];
        assert_eq!(initials(&["lead".into(), "writer".into()], &roles), ["LD", "WR"]);
    }
}

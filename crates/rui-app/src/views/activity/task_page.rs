//! The task page: one task's header, team strip, tabs and composer, shown in place of the board.

use super::teams::can_stop;
use super::flow::{is_planned, lane_row};
use super::{Tab, Thread, ThreadsView, effort_section, elapsed, icon, live_row, first_line, mono, role_name, scroll_sig, section};
use crate::api::{Node, Thread as FlowThread};
use crate::theme::Theme;
use crate::views::character::character;
use crate::views::library::danger_button;
use crate::views::status::{self, Word};
use crate::views::trace::trace;
use crate::views::{chip, tag};
use super::char_state;
use gpui::{AnyElement, Context, Div, FontWeight, div, prelude::*, px};
use rui_core::TaskStatus;

const RESUME_TEXT: &str = "Resume where you left off.";

/// Header counts: came in, agent steps done, went out, planned next.
pub fn thread_counts(nodes: &[Node]) -> [usize; 4] {
    let mut c = [0; 4];
    for n in nodes {
        if is_planned(n) {
            c[3] += 1;
            continue;
        }
        match lane_row(&n.lane) {
            0 => c[0] += 1,
            2 => c[2] += 1,
            _ if n.status == "done" => c[1] += 1,
            _ => {}
        }
    }
    c
}

/// The non-zero counts as (index, "N came in") pairs.
pub fn count_parts(c: [usize; 4]) -> Vec<(usize, String)> {
    let words = ["came in", "agent steps done", "went out", "planned next"];
    (0..4).filter(|&i| c[i] > 0).map(|i| (i, format!("{} {}", c[i], words[i]))).collect()
}

/// The counts line with a small coloured dot before each part.
fn counts_line(nodes: &[Node], th: &Theme) -> Option<AnyElement> {
    let parts = count_parts(thread_counts(nodes));
    if parts.is_empty() {
        return None;
    }
    let colors = [th.wait.base, th.done.base, th.run.base, th.muted];
    let item = |(i, text): (usize, String)| {
        let dot = div().flex_none().size(px(6.)).rounded_full().bg(colors[i]);
        div().flex().items_center().gap(px(6.)).text_size(px(12.5)).text_color(th.fg_2).child(dot).child(text)
    };
    Some(div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(4.)).children(parts.into_iter().map(item)).into_any_element())
}

/// The state word in a pill with a coloured dot: "Needs you", "Working", "Waiting", "Done", "Stopped".
fn state_chip(word: Word, th: &Theme) -> Div {
    let st = status::style(word, th);
    let dot = div().flex_none().size(px(6.)).rounded_full().bg(st.base);
    let chip = div().flex().items_center().gap(px(6.)).h(px(22.)).px(px(10.)).rounded(px(7.));
    let chip = chip.text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).bg(st.dim).text_color(st.fg);
    chip.border_1().border_color(st.rim).child(dot).child(word.label())
}

impl ThreadsView {
    /// The whole page for one task, replacing the board.
    pub(super) fn task_page(&mut self, t: &Thread, all: &[Thread], now: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (events, changes) = self.task_data(t.task.id, cx);
        let results = self.store.read(cx).results.get(&t.task.id).cloned().unwrap_or_default();
        if self.auto_results && !results.is_empty() {
            self.auto_results = false;
            self.set_tab(Tab::Results);
        }
        let rows = trace(&events);
        let live = self.store.read(cx).live.task(t.task.id).cloned();
        let sig = scroll_sig(rows.len(), live.as_ref());
        if self.tab == Tab::Trace && sig != self.last_len {
            self.last_len = sig;
            self.scroll.scroll_to_bottom();
        }
        let flow = self.store.read(cx).thread_cache.get(&t.task.id).cloned().filter(|_| t.task.parent_id.is_none());
        let body = match self.tab {
            Tab::Flow => self.flow(flow.as_ref(), th, cx),
            Tab::Trace => {
                let mut items = self.trace_rows(&rows, th, cx);
                items.extend(live.as_ref().map(|l| live_row(l, th)));
                section("Trace", items)
            }
            Tab::Changes => section("What it changed", super::change_cards(&changes, th)),
            Tab::Files => self.files_tab(t.task.id, &rows, &changes, th, cx),
            Tab::Results => section("Files it produced", self.result_rows(&results, th, cx)),
        };
        let scroll = div().id("task-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll);
        let composer = (t.task.status != TaskStatus::Done).then(|| self.composer(t, th, cx));
        let details = self.details.then(|| self.details_section(t, all, now, th, cx));
        let has_flow = t.task.parent_id.is_none();
        let switch = self.tabs(has_flow, !results.is_empty(), has_flow, th, cx);
        let page = div().id("task-page").relative().flex().size_full().overflow_hidden();
        if !has_flow {
            let scroll = scroll.flex().flex_col().gap(px(20.)).px(px(40.)).py(px(20.)).child(body);
            let face = character(char_state(&t.task.status, live.as_ref()), px(58.), ("task-rui", t.task.id as usize), th.client_color(t.task.client_id), th);
            let tabs = div().id("task-tabs-wrap").px(px(40.)).pt(px(16.)).child(switch);
            let col = div().flex().flex_col().flex_1().min_w_0().child(self.crumbs(t, th, cx)).child(self.page_head(t, face, th, cx));
            return page.child(col.children(details).child(tabs).child(scroll).children(composer)).into_any_element();
        }
        let scroll = scroll.flex().flex_col().gap(px(20.)).p(px(24.)).child(body);
        let card = div().id("task-body-card").flex().flex_col().flex_1().min_h_0().overflow_hidden().rounded(px(16.)).bg(th.surface).shadow(th.card_shadow());
        let card = card.child(scroll).children(composer);
        let card = div().flex().flex_col().flex_1().min_h_0().px(px(30.)).pt(px(14.)).pb(px(14.)).child(card);
        let head = div().px(px(30.)).pt(px(4.)).child(self.head_card(t, flow.as_ref(), switch, th, cx));
        let col = div().flex().flex_col().flex_1().min_w_0().min_h_0().h_full().child(self.crumbs(t, th, cx)).child(head);
        let col = col.children(details).child(card);
        page.child(col).children(self.inspector_view(flow.as_ref(), now, th, cx)).into_any_element()
    }

    /// "Client > Threads > title"; Threads goes back to the board.
    fn crumbs(&self, t: &Thread, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let text = |s: String, color| div().text_size(px(12.5)).text_color(color).child(s);
        let sep = || icon("chevron", 11., th.muted).into_any_element();
        let hover = th.surface_2;
        let threads = div().id("task-crumb-threads").px(px(6.)).py(px(2.)).rounded(px(6.)).text_size(px(12.5)).text_color(th.fg_2);
        let threads = threads.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let mut row = div().flex().items_center().gap(px(6.));
        if !t.client.is_empty() {
            row = row.child(text(t.client.clone(), th.muted)).child(sep());
        }
        let title = text(t.task.title.clone(), th.fg).flex_1().min_w_0().truncate();
        let row = row.child(threads.child("Threads")).child(sep()).child(title);
        div().id("task-crumbs").flex().flex_none().px(px(30.)).pt(px(14.)).child(row.min_w_0()).into_any_element()
    }

    /// Eyebrow, title, status word with its reason, model and time, and Stop or Resume on the right.
    fn page_head(&self, t: &Thread, face: AnyElement, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let eyebrow = format!("{} \u{b7} {}", t.client, role_name(&t.task.role)).to_uppercase();
        let eyebrow = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(eyebrow);
        let title = div().text_size(px(26.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(t.task.title.clone());
        let reason = first_line(&t.reason, 120);
        let reason = (!reason.is_empty()).then(|| div().min_w_0().truncate().text_size(px(12.5)).text_color(th.fg_2).child(reason));
        let word = tag(status::style(t.word, th), t.word.label());
        let label = if self.details { "Hide details" } else { "Details" };
        let toggle = chip("task-details", label, cx).on_click(cx.listener(|this, _, _, cx| this.toggle_details(cx)));
        let meta = div().id("task-meta").flex().items_center().gap(px(8.)).min_w_0().child(word).children(reason).child(toggle);
        let info = div().id("task-info").flex().flex_col().flex_1().min_w_0().gap(px(6.));
        let info = info.child(eyebrow).child(title).child(meta);
        let actions = div().id("task-actions").flex().items_center().gap(px(8.));
        let actions = actions.children(self.page_action(t, th, cx)).children(self.abandon_control(t, th, cx));
        let row = div().id("task-head").flex().flex_none().items_start().gap(px(14.)).px(px(40.)).pt(px(8.));
        row.child(face).child(info).child(actions).into_any_element()
    }

    /// Top-level thread header: state chip and id, actions and switch, title, story and counts in one card.
    fn head_card(&self, t: &Thread, flow: Option<&FlowThread>, switch: AnyElement, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let label = if self.details { "Hide details" } else { "Details" };
        let hover = th.fg;
        let toggle = div().id("task-details").text_size(px(11.5)).text_color(th.muted).cursor_pointer().hover(move |s| s.text_color(hover));
        let toggle = toggle.on_click(cx.listener(|this, _, _, cx| this.toggle_details(cx))).child(label);
        let id = div().flex().items_center().h(px(22.)).px(px(8.)).rounded(px(7.)).bg(th.surface_2);
        let id = id.child(mono(11., th.fg_2, format!("THR-{}", t.task.id)));
        let left = div().flex().items_center().gap(px(8.)).child(state_chip(t.word, th)).child(id).child(toggle);
        let actions = div().id("task-actions").flex().items_center().gap(px(8.));
        let actions = actions.children(self.page_action(t, th, cx)).children(self.abandon_control(t, th, cx));
        let right = div().flex().items_center().gap(px(12.)).child(actions).child(switch);
        let top = div().flex().items_center().justify_between().gap(px(12.)).child(left).child(right);
        let title = div().text_size(px(26.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(t.task.title.clone());
        let story = flow.and_then(|f| f.story.clone()).filter(|s| !s.trim().is_empty());
        let story = story.unwrap_or_else(|| first_line(&t.reason, 200));
        let story = (!story.is_empty()).then(|| div().max_w(px(860.)).text_size(px(14.)).text_color(th.fg_2).child(story));
        let counts = flow.and_then(|f| counts_line(&f.nodes, th));
        let card = div().id("task-head-card").flex().flex_col().gap(px(10.)).p(px(24.)).rounded(px(16.)).bg(th.surface);
        card.shadow(th.card_shadow()).child(top).child(title).children(story).children(counts).into_any_element()
    }

    fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details = !self.details;
        cx.notify();
    }

    /// Model, time and pod line, then the team strip and the effort meter.
    fn details_section(&self, t: &Thread, all: &[Thread], now: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let parts = [t.model.clone(), Some(elapsed(&t.task.updated_at, now)), t.pod_line.clone()];
        let line = parts.into_iter().flatten().collect::<Vec<_>>().join(" \u{b7} ");
        let line = div().px(px(40.)).pt(px(8.)).text_size(px(11.5)).text_color(th.muted).child(line);
        let effort = div().px(px(40.)).pt(px(8.)).child(effort_section(&t.task, th));
        div().id("task-details-section").flex().flex_col().child(line).children(self.team_strip(t, all, th, cx)).child(effort).into_any_element()
    }

    /// "Abandon" for a task that isn't done; a second click drops the whole task (its top-level
    /// task and every teammate), even from a teammate's page.
    fn abandon_control(&self, t: &Thread, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if t.word == Word::Done {
            return None;
        }
        let id = t.task.parent_id.unwrap_or(t.task.id);
        if !self.abandon_armed {
            let arm = chip("task-abandon", "Abandon", cx).on_click(cx.listener(|this, _, _, cx| this.arm_abandon(cx)));
            return Some(arm.into_any_element());
        }
        let go = danger_button("task-abandon-now", "Click again to abandon", true, th);
        Some(go.on_click(cx.listener(move |this, _, _, cx| this.abandon(id, cx))).into_any_element())
    }

    fn arm_abandon(&mut self, cx: &mut Context<Self>) {
        self.abandon_armed = true;
        cx.notify();
    }

    /// Stop the whole team and mark the task dropped; it moves to Done as "Dropped by you".
    fn abandon(&mut self, id: i64, cx: &mut Context<Self>) {
        self.abandon_armed = false;
        self.store.update(cx, |s, cx| s.drop_task(id, cx));
        cx.notify();
    }

    /// Stop while it can run, Resume team once stopped.
    fn page_action(&self, t: &Thread, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = t.task.id;
        if t.word == Word::Stopped {
            let resume = chip("task-resume", "Resume team", cx).on_click(cx.listener(move |this, _, _, cx| this.resume_team(id, cx)));
            return Some(resume.into_any_element());
        }
        if !matches!(t.word, Word::Working | Word::Waiting) || !can_stop(&t.task.status) {
            return None;
        }
        Some(self.stop_control(th, cx))
    }

    /// "Stop" first; once clicked, an optional one-line "Why?" and a "Stop now" confirm.
    fn stop_control(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if !self.stop_open {
            let stop = danger_button("task-stop", "Stop", true, th);
            return stop.on_click(cx.listener(|this, _, _, cx| this.open_stop(cx))).into_any_element();
        }
        let why = div().id("task-stop-why").w(px(240.)).child(self.why.clone());
        let now = danger_button("task-stop-now", "Stop now", true, th).on_click(cx.listener(|this, _, _, cx| this.stop_now(cx)));
        div().id("task-stop-box").flex().items_center().gap(px(8.)).child(why).child(now).into_any_element()
    }

    fn open_stop(&mut self, cx: &mut Context<Self>) {
        self.stop_open = true;
        cx.notify();
    }

    /// Stop the open task from the task page, with the optional reason.
    pub(super) fn stop_now(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.open else { return };
        let reason = Some(self.why.read(cx).content().trim().to_string()).filter(|r| !r.is_empty());
        self.stop_open = false;
        self.why.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.stop_task(id, "Task page", reason, cx));
        cx.notify();
    }

    fn resume_team(&mut self, id: i64, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.instruct_task(id, RESUME_TEXT.to_string(), cx));
    }

    /// One chip per subtask of a top-level task: role name, status word, reason.
    fn team_strip(&self, t: &Thread, all: &[Thread], th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let kids: Vec<&Thread> = all.iter().filter(|k| k.task.parent_id == Some(t.task.id)).collect();
        if t.task.parent_id.is_some() || kids.is_empty() {
            return None;
        }
        let chips: Vec<AnyElement> = kids.into_iter().map(|k| self.member_chip(k, th, cx)).collect();
        let row = div().id("task-team").flex().flex_wrap().gap(px(8.)).px(px(40.)).pt(px(14.)).children(chips);
        Some(row.into_any_element())
    }

    fn member_chip(&self, k: &Thread, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = k.task.id;
        let name = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(k.name.clone());
        let top = div().flex().items_center().gap(px(8.)).child(name).child(tag(status::style(k.word, th), k.word.label()));
        let reason = (!k.reason.is_empty()).then(|| div().max_w(px(260.)).truncate().text_size(px(11.)).text_color(th.muted).child(k.reason.clone()));
        let hover = th.surface_2;
        let card = div().id(("team-chip", id as usize)).flex().flex_col().gap(px(3.)).px(px(10.)).py(px(7.)).rounded(px(9.));
        let card = card.bg(th.surface).border_1().border_color(th.hairline).cursor_pointer().hover(move |s| s.bg(hover));
        card.on_click(cx.listener(move |this, _, _, cx| this.open_thread(id, cx))).child(top).children(reason).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(lane: &str, status: &str, at: &str) -> Node {
        Node { id: "n".into(), lane: lane.into(), status: status.into(), started_at: at.into(), ..Default::default() }
    }

    #[test]
    fn counts_by_lane_and_plan() {
        let t = "2026-11-18 10:00:00";
        let nodes = [node("in", "done", t), node("work", "done", t), node("work", "running", t), node("out", "done", t), node("work", "planned", "")];
        assert_eq!(thread_counts(&nodes), [1, 1, 1, 1]);
        assert_eq!(thread_counts(&[]), [0; 4]);
    }

    #[test]
    fn zero_counts_are_left_out() {
        let parts = count_parts([2, 0, 1, 0]);
        assert_eq!(parts, vec![(0, "2 came in".to_string()), (2, "1 went out".to_string())]);
    }
}

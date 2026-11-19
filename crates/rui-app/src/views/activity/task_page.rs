//! The task page: one task's header, team strip, tabs and composer, shown in place of the board.

use super::teams::can_stop;
use super::{Tab, Thread, ThreadsView, effort_section, elapsed, icon, live_row, first_line, role_name, scroll_sig, section};
use crate::theme::Theme;
use crate::views::character::character;
use crate::views::library::danger_button;
use crate::views::status::{self, Word};
use crate::views::trace::trace;
use crate::views::{chip, tag};
use super::char_state;
use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use rui_core::TaskStatus;

const RESUME_TEXT: &str = "Resume where you left off.";

/// The thread's story, one line under the header.
fn story_line(story: Option<&str>, th: &Theme) -> Option<AnyElement> {
    let s = story.filter(|s| !s.trim().is_empty())?;
    let line = div().id("task-story").px(px(40.)).pt(px(8.)).truncate().text_size(px(12.5)).text_color(th.fg_2);
    Some(line.child(s.to_string()).into_any_element())
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
            Tab::Flow => section("Flow", vec![self.flow(flow.as_ref(), th, cx)]),
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
        let scroll = scroll.flex().flex_col().gap(px(20.)).px(px(40.)).py(px(20.)).child(body);
        let composer = (t.task.status != TaskStatus::Done).then(|| self.composer(t, th, cx));
        let face = character(char_state(&t.task.status, live.as_ref()), px(58.), ("task-rui", t.task.id as usize), th.client_color(t.task.client_id), th);
        let head = self.page_head(t, face, th, cx);
        let story = flow.as_ref().and_then(|f| story_line(f.story.as_deref(), th));
        let details = self.details.then(|| self.details_section(t, all, now, th, cx));
        let has_flow = t.task.parent_id.is_none();
        let tabs = div().id("task-tabs-wrap").px(px(40.)).pt(px(16.)).child(self.tabs(has_flow, !results.is_empty(), th, cx));
        let inspector = self.inspector_view(flow.as_ref(), th, cx);
        let page = div().id("task-page").relative().flex().flex_col().size_full().child(self.back_button(th, cx)).child(head);
        page.children(details).children(story).child(tabs).child(scroll).children(composer).children(inspector).into_any_element()
    }

    fn back_button(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (hover, muted) = (th.surface_2, th.fg_2);
        let back = div().id("task-back").flex().items_center().gap(px(6.)).h(px(30.)).px(px(10.)).rounded(px(8.));
        let back = back.text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(muted).cursor_pointer();
        let back = back.hover(move |s| s.bg(hover)).on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let row = back.child(icon("chevron", 12., th.fg_2).into_any_element()).child("Board");
        div().id("task-back-row").flex().flex_none().px(px(30.)).pt(px(14.)).child(row).into_any_element()
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

//! Threads board pieces: flat bands of agent cards, Stop buttons, the Stop all confirm and the chart data.

use super::teams::{DONE_CAP, can_stop, cap_items, stop_targets};
use super::timeline::{Chart, Line, ROWS, clip, trajectory, window_start, worth_drawing};
use super::{Thread, ThreadsView, parse_secs};
use crate::theme::Theme;
use crate::views::library::{confirm_modal, danger_button};
use crate::store::Store;
use crate::views::status::Word;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Stateful, div, prelude::*, px};
use rui_core::{Task, TaskStatus};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Where a task's row is in its stop animation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Phase {
    Idle,
    Leaving,
    Gone,
}

/// "Stops N teams: A, B. They keep what they've done so far."
fn stop_all_body(s: &Store) -> String {
    // Same rule as the daemon's /stop-all, so the list matches exactly what gets stopped.
    let live = |t: &&Task| !matches!(t.status, TaskStatus::Done | TaskStatus::Stuck);
    let titles: Vec<&str> = s.tasks.iter().filter(|t| t.parent_id.is_none()).filter(live).map(|t| t.title.as_str()).collect();
    let noun = if titles.len() == 1 { "team" } else { "teams" };
    format!("Stops {} {noun}: {}. They keep what they've done so far.", titles.len(), titles.join(", "))
}

fn find(tasks: &[Task], id: i64) -> Option<&Task> {
    tasks.iter().find(|t| t.id == id)
}

impl ThreadsView {
    /// Every band's cards, one per agent; Done is newest first.
    pub(super) fn bands(&self, threads: &[Thread]) -> HashMap<Word, Vec<Thread>> {
        let mut out: HashMap<Word, Vec<Thread>> = ROWS.iter().map(|r| (r.0, Vec::new())).collect();
        for t in threads {
            if let Some(band) = out.get_mut(&t.word) {
                band.push(t.clone());
            }
        }
        if let Some(done) = out.get_mut(&Word::Done) {
            done.sort_by_key(|t| Reverse(parse_secs(&t.task.updated_at).unwrap_or(0)));
        }
        out
    }

    /// Lines and counts for the timeline above the bands.
    pub(super) fn chart(&self, threads: &[Thread], now: i64, th: &Theme, cx: &Context<Self>) -> Chart {
        let s = self.store.read(cx);
        let from = window_start(now);
        let line = |t: &Thread| {
            let pts = clip(trajectory(&t.task, &s.events, t.word, now), from);
            if !worth_drawing(&pts, from, t.word) {
                return None;
            }
            Some(Line { pts, color: th.client_color(t.task.client_id) })
        };
        let counts = ROWS.map(|(c, _)| threads.iter().filter(|t| t.word == c).count());
        let clients = s.clients.iter().map(|c| (c.name.clone(), th.client_color(c.id))).collect();
        Chart { lines: threads.iter().filter_map(line).collect(), counts, now, clients }
    }

    /// Drop stop animations once the task has really stopped, or after a few seconds.
    pub(super) fn prune_stopping(&mut self, cx: &Context<Self>) {
        let tasks = &self.store.read(cx).tasks;
        let open = |id: &i64| find(tasks, *id).is_some_and(|t| !matches!(t.status, TaskStatus::Stuck | TaskStatus::Done));
        self.stopping.retain(|id, at| at.elapsed() < Duration::from_secs(4) && open(id));
    }

    pub(super) fn phase(&self, id: i64) -> Phase {
        match self.stopping.get(&id) {
            None => Phase::Idle,
            Some(at) if at.elapsed() < Duration::from_millis(super::LEAVE_MS) => Phase::Leaving,
            Some(_) => Phase::Gone,
        }
    }

    fn begin_stopping(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.stopping.extend(ids.into_iter().map(|id| (id, now)));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(super::LEAVE_MS + 40)).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Stop a task (and its team), play the leave animation, and toast the count.
    fn stop_group(&mut self, id: i64, cx: &mut Context<Self>) {
        let targets = stop_targets(&self.store.read(cx).tasks, id);
        self.begin_stopping(targets, cx);
        self.store.update(cx, |s, cx| s.stop_task(id, "Work", cx));
    }

    pub(super) fn cancel_stop_all(&mut self, cx: &mut Context<Self>) {
        self.confirm_all = false;
        cx.notify();
    }

    pub(super) fn accept_stop_all(&mut self, cx: &mut Context<Self>) {
        self.confirm_all = false;
        let tasks = self.store.read(cx).tasks.clone();
        let open = |t: &&Task| t.parent_id.is_none() && !matches!(t.status, TaskStatus::Done | TaskStatus::Stuck);
        let ids: Vec<i64> = tasks.iter().filter(open).flat_map(|t| stop_targets(&tasks, t.id)).collect();
        self.begin_stopping(ids, cx);
        self.store.update(cx, |s, cx| s.stop_all(cx));
    }

    pub(super) fn confirm_view(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.confirm_all {
            return None;
        }
        let body = stop_all_body(self.store.read(cx));
        Some(confirm_modal("Stop all teams?", &body, "Stop all", true, Self::cancel_stop_all, Self::accept_stop_all, th, cx))
    }

    /// Danger "Stop all" button for the page head, only while something can be stopped.
    pub(super) fn stop_all_button(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let running = self.store.read(cx).tasks.iter().any(|t| can_stop(&t.status));
        running.then(|| {
            let b = danger_button("threads-stop-all", "Stop all", true, th);
            let on_click = cx.listener(|this, _, _, cx| {
                this.confirm_all = true;
                cx.notify();
            });
            b.on_click(on_click).into_any_element()
        })
    }

    /// Small red button on a card.
    pub(super) fn stop_pill(&self, id: String, task_id: i64, th: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let pill = div().id(SharedString::from(id)).flex().flex_none().items_center().h(px(22.)).px(px(9.)).rounded(px(6.));
        let pill = pill.text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).cursor_pointer();
        let pill = pill.bg(th.block.dim).text_color(th.block.fg).border_1().border_color(th.block.rim);
        pill.child("Stop").on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.stop_group(task_id, cx);
        }))
    }

    fn toggle_link(&self, id: &'static str, text: String, th: &Theme, cx: &mut Context<Self>, flip: fn(&mut Self)) -> AnyElement {
        let hover = th.fg_2;
        div()
            .id(id)
            .flex_none()
            .px(px(6.))
            .py(px(6.))
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(th.muted)
            .cursor_pointer()
            .hover(move |s| s.text_color(hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(this);
                cx.notify();
            }))
            .child(text)
            .into_any_element()
    }

    /// Done shows the newest cards with a "Show N done" toggle.
    fn done_rows(&self, items: &[Thread], now: i64, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let refs: Vec<&Thread> = items.iter().collect();
        let (shown, hidden) = cap_items(refs, DONE_CAP, self.show_done);
        let mut rows: Vec<AnyElement> = shown.iter().enumerate().map(|(i, t)| self.moving_row(t, i, now, th, cx)).collect();
        let text = if self.show_done { "Show fewer".to_string() } else { format!("Show {hidden} done") };
        if hidden > 0 || (self.show_done && items.len() > DONE_CAP) {
            rows.push(self.toggle_link("done-toggle", text, th, cx, |v| v.show_done = !v.show_done));
        }
        rows
    }

    pub(super) fn column(&self, col: Word, items: &[Thread], all: &[Thread], now: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = match col {
            Word::Done => self.done_rows(items, now, th, cx),
            _ => items.iter().enumerate().map(|(i, t)| self.moving_row(t, i, now, th, cx)).collect(),
        };
        let ghosts = self.leaving(col, all, now, th, cx);
        let blank = rows.is_empty() && ghosts.is_empty();
        let none = blank.then(|| div().px(px(8.)).py(px(16.)).text_size(px(11.5)).text_color(th.muted).child("Empty"));
        rows.extend(ghosts);
        let list = div().id(("threads-col-list", col as usize)).flex().flex_col().p(px(8.));
        let list = list.children(rows).children(none);
        div()
            .id(("threads-col", col as usize))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h(px(300.))
            .rounded(px(12.))
            .bg(th.well)
            .border_1()
            .border_color(th.hairline)
            .child(super::column_head(col.label(), items.len(), th))
            .child(list)
            .into_any_element()
    }
}

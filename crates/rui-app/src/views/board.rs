//! Work page: commitments laid out in lanes, left to right.

use super::section::{Sec, section_top};
use super::card;
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use gpui::{Context, Entity, FontWeight, Window, div, prelude::*, px};
use rui_core::{Task, TaskRun, TaskStatus};

/// Lane names, left to right.
const LANES: [&str; 5] = ["Taken in", "Shaped", "Being built", "Your call", "Delivered"];

/// Tasks grouped into lanes by status.
pub struct BoardView {
    store: Entity<Store>,
}

impl BoardView {
    /// Build the board view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        Self { store }
    }
}

/// Which lane a status belongs to.
fn lane_of(s: TaskStatus) -> usize {
    match s {
        TaskStatus::New => 0,
        TaskStatus::Clarifying | TaskStatus::Split => 1,
        TaskStatus::Open | TaskStatus::InProgress => 2,
        TaskStatus::WaitingAnswer | TaskStatus::Stuck => 3,
        TaskStatus::Done => 4,
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn seat_text(run: &TaskRun) -> String {
    let seat = match run.seat.as_str() {
        "subscription" => "subscription",
        "api_key" => "api key",
        _ => "unknown",
    };
    format!("{} · {seat}", run.model.as_deref().unwrap_or("—"))
}

fn meta_text(t: &Task, client: &str) -> String {
    let base = format!("{client} · {}", capitalize(t.role.as_str()));
    match t.parent_id {
        Some(p) => format!("{base} · #{p}"),
        None => base,
    }
}

fn task_card(
    t: &Task,
    client: &str,
    run: Option<&TaskRun>,
    th: &Theme,
    cx: &gpui::App,
) -> impl IntoElement {
    let meta = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .mt(px(6.))
        .text_size(px(10.5))
        .text_color(th.muted)
        .child(div().size(px(8.)).flex_none().rounded_full().bg(th.client_color(t.client_id)))
        .child(meta_text(t, client));
    let note = run.map(|r| div().text_size(px(10.5)).text_color(th.muted).child(seat_text(r)));
    let id = t.id;
    card(cx)
        .id(("task-card", t.id as usize))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            let store = cx.global::<store::StoreHandle>().0.clone();
            store.update(cx, |s, cx| s.open_thread(id, cx));
        })
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(th.fg)
                .line_height(px(16.2))
                .child(t.title.clone()),
        )
        .child(meta)
        .children(note)
}

fn lane_header(i: usize, count: usize, th: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(12.))
        .pt(px(12.))
        .pb(px(8.))
        .child(
            div()
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(th.fg)
                .child(LANES[i]),
        )
        .child(
            div()
                .font_family("Menlo")
                .text_size(px(11.))
                .text_color(th.muted)
                .child(count.to_string()),
        )
}

fn lane(i: usize, cards: Vec<gpui::AnyElement>, th: &Theme) -> impl IntoElement {
    let count = cards.len();
    let body = div().id(("lane-body", i)).flex().flex_col().gap(px(6.)).px(px(8.)).pb(px(8.));
    let body = if cards.is_empty() {
        body.child(div().p(px(4.)).text_size(px(11.5)).text_color(th.muted).child("Empty"))
    } else {
        body.children(cards)
    };
    div()
        .id(("lane", i))
        .flex_1()
        .min_w(px(168.))
        .bg(th.well)
        .rounded(px(12.))
        .border_1()
        .border_color(th.hairline)
        .flex()
        .flex_col()
        .child(lane_header(i, count, th))
        .child(body)
}

fn lead_line(open: usize, th: &Theme) -> impl IntoElement {
    div()
        .id("work-lead")
        .pb(px(20.))
        .text_size(px(15.))
        .text_color(th.fg_2)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(19.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(th.fg)
                        .child(open.to_string()),
                )
                .child("open commitments. They only move left to right."),
        )
}

impl Render for BoardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let s = self.store.read(cx);
        let open = s.tasks.iter().filter(|t| t.status != TaskStatus::Done).count();
        let mut cards: Vec<Vec<gpui::AnyElement>> = (0..LANES.len()).map(|_| Vec::new()).collect();
        for t in &s.tasks {
            let name = s.clients.iter().find(|c| c.id == t.client_id).map_or("", |c| c.name.as_str());
            cards[lane_of(t.status)]
                .push(task_card(t, name, s.runs.get(&t.id), &th, cx).into_any_element());
        }
        let lanes: Vec<_> = cards.into_iter().enumerate().map(|(i, c)| lane(i, c, &th)).collect();
        div()
            .flex()
            .flex_col()
            .size_full()
            .pr(px(40.))
            .child(section_top(Sec::Work, Some(lead_line(open, &th).into_any_element()), None, cx))
            .child(
                div()
                    .id("work-lanes")
                    .flex()
                    .gap(px(8.))
                    .pb(px(28.))
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .children(lanes),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_a_lane() {
        for s in TaskStatus::ALL {
            assert!(lane_of(*s) < LANES.len());
        }
        assert_eq!(lane_of(TaskStatus::Stuck), 3);
        assert_eq!(lane_of(TaskStatus::Done), 4);
    }
}

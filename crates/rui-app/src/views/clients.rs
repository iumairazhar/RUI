//! Clients page: one card per client.

use super::activity::{Col, col_of};
use super::card;
use super::character::{CharState, character};
use super::section::{Sec, section_top};
use crate::store::StoreHandle;
use crate::theme::{Theme, theme};
use gpui::{AnyElement, App, FontWeight, div, prelude::*, px};
use rui_core::{Client, Task, TaskStatus};

/// Top-level tasks of a client, and how many of them are in the loop.
fn counts(client: i64, tasks: &[Task]) -> (usize, usize) {
    let mine = tasks.iter().filter(|t| t.client_id == client && t.parent_id.is_none());
    mine.fold((0, 0), |(a, r), t| (a + 1, r + (t.status == TaskStatus::InProgress) as usize))
}

fn needs_you(client: i64, tasks: &[Task]) -> bool {
    tasks.iter().any(|t| t.client_id == client && col_of(&t.status) == Some(Col::Needs))
}

fn kv(label: &str, value: usize, th: &Theme) -> AnyElement {
    let name = div().text_size(px(11.5)).text_color(th.muted).child(label.to_string());
    let num = div().font_family("Menlo").text_size(px(12.5)).text_color(th.fg).child(value.to_string());
    div().flex().justify_between().child(name).child(num).into_any_element()
}

fn client_card(c: &Client, tasks: &[Task], cx: &App) -> AnyElement {
    let th = theme(cx);
    let (all, running) = counts(c.id, tasks);
    let state = match (running, needs_you(c.id, tasks)) {
        (0, true) => CharState::Blocked,
        (0, false) => CharState::Standing,
        _ => CharState::Doing,
    };
    let face = character(state, px(34.), ("client-rui", c.id as usize), th.client_color(c.id), th);
    let name = div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(c.name.clone());
    let head = div().flex().items_center().gap(px(10.)).child(face).child(name);
    let path = div().truncate().text_size(px(11.)).text_color(th.muted).child(c.repo_path.clone());
    let stats = div().flex().flex_col().gap(px(4.)).child(kv("Tasks", all, th)).child(kv("In the loop", running, th));
    let body = div().flex().flex_col().gap(px(12.)).child(head).child(stats).child(path);
    let card = card(cx).id(("client-card", c.id as usize)).w(px(280.)).p(px(16.));
    card.child(body).into_any_element()
}

/// The Clients tab of the Clients section.
pub fn page(cx: &App) -> AnyElement {
    let store = cx.global::<StoreHandle>().0.read(cx);
    let cards: Vec<AnyElement> = store.clients.iter().map(|c| client_card(c, &store.tasks, cx)).collect();
    let grid = div().id("clients-grid").flex().flex_wrap().gap(px(12.)).pb(px(28.)).children(cards);
    let scroll = div().id("clients-scroll").flex_1().min_h_0().overflow_y_scroll().child(grid);
    let top = section_top(Sec::Clients, None, None, cx);
    div().id("clients-page").flex().flex_col().size_full().pr(px(40.)).child(top).child(scroll).into_any_element()
}

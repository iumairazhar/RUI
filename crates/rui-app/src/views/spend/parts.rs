//! Spend page pieces: the daily bar chart and the client, thread and seat tables.

use super::SpendView;
use super::fmt::{bar_heights, fill_days, money, short_date, today, tokens};
use crate::api::{ClientCost, Costs, SeatCost, ThreadCost};
use crate::store::Store;
use crate::theme::Theme;
use crate::views::activity::section;
use crate::views::card;
use gpui::{AnyElement, App, Context, Div, Entity, FontWeight, div, prelude::*, px};

const CHART_H: f32 = 90.;
const NUM_W: f32 = 90.;
const ESTIMATE: &str = "Estimated by the CLI on subscription seats.";

/// Chart, then the client, thread and seat tables.
pub fn spend_sections(c: &Costs, days: u32, store: &Entity<Store>, th: &Theme, cx: &mut Context<SpendView>) -> Vec<AnyElement> {
    let clients = c.by_client.iter().map(|r| client_row(r, th)).collect();
    let threads = c.by_thread.iter().enumerate().map(|(i, r)| thread_row(i, r, store, th, cx)).collect();
    vec![
        section("Spend per day", vec![chart(c, days, th, cx)]),
        section("By client", table(&["Client", "Threads", "Tokens", "Spend"], clients, th)),
        section("Top threads", table(&["Thread", "", "Tokens", "Spend"], threads, th)),
        seats_section(&c.by_seat, th),
    ]
}

/// Div bars scaled to the biggest day, with the first, middle and last dates under them.
fn chart(c: &Costs, days: u32, th: &Theme, cx: &App) -> AnyElement {
    let filled = fill_days(&c.by_day, today(), days);
    let values: Vec<f64> = filled.iter().map(|d| d.1).collect();
    let bars = bar_heights(&values, CHART_H).into_iter().map(|h| bar(h, th));
    let plot = div().flex().items_end().gap(px(2.)).h(px(CHART_H)).border_b_1().border_color(th.hairline).children(bars);
    let label = |i: usize| filled.get(i).map(|d| short_date(d.0)).unwrap_or_default();
    let axis = [label(0), label(filled.len() / 2), label(filled.len().saturating_sub(1))];
    let axis = div().flex().justify_between().pt(px(6.)).children(axis.map(|t| muted(t, th)));
    let top = values.iter().copied().fold(0., f64::max);
    let peak = muted(format!("Peak day {}", money(top)), th);
    card(cx).p(px(16.)).max_w(px(760.)).flex().flex_col().gap(px(8.)).child(peak).child(plot).child(axis).into_any_element()
}

fn bar(h: f32, th: &Theme) -> Div {
    let el = div().flex_1().max_w(px(22.)).rounded_t(px(3.)).bg(th.accent_fill);
    if h > 0. { el.h(px(h.max(2.))) } else { el.h(px(1.)).bg(th.hairline) }
}

fn muted(text: impl Into<String>, th: &Theme) -> Div {
    div().text_size(px(11.)).text_color(th.muted).child(text.into())
}

/// Header row, then the rows; a single note when there are none.
fn table(head: &[&str], rows: Vec<AnyElement>, th: &Theme) -> Vec<AnyElement> {
    if rows.is_empty() {
        return vec![muted("Nothing in this range.", th).into_any_element()];
    }
    let cells: Vec<String> = head.iter().map(|h| h.to_string()).collect();
    let head = row_of(cells, th).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted);
    let body = div().flex().flex_col().max_w(px(760.)).children(rows);
    vec![div().flex().flex_col().max_w(px(760.)).child(head).child(body).into_any_element()]
}

/// A row: the first cell takes the room, the rest are right-aligned numbers.
fn row_of(cells: Vec<String>, th: &Theme) -> Div {
    let mut it = cells.into_iter();
    let first = div().flex_1().min_w_0().truncate().child(it.next().unwrap_or_default());
    let nums = it.map(|c| div().w(px(NUM_W)).flex().justify_end().child(c));
    div().flex().items_center().gap(px(12.)).px(px(12.)).py(px(8.)).border_b_1().border_color(th.hairline).child(first).children(nums)
}

fn client_row(r: &ClientCost, th: &Theme) -> AnyElement {
    let cells = vec![r.name.clone(), r.tasks.to_string(), tokens(r.tokens), money(r.usd)];
    row_of(cells, th).text_size(px(12.5)).text_color(th.fg).into_any_element()
}

/// A thread row; a click opens the thread.
fn thread_row(i: usize, r: &ThreadCost, store: &Entity<Store>, th: &Theme, cx: &mut Context<SpendView>) -> AnyElement {
    let cells = vec![r.title.clone(), r.client.clone(), tokens(r.tokens), money(r.usd)];
    let (id, store, hover) = (r.task_id, store.clone(), th.surface_2);
    let row = row_of(cells, th).text_size(px(12.5)).text_color(th.fg).id(("spend-thread", i)).cursor_pointer();
    let row = row.hover(move |s| s.bg(hover));
    row.on_click(cx.listener(move |_, _, _, cx| store.update(cx, |s, cx| s.open_thread(id, cx)))).into_any_element()
}

fn seat_row(r: &SeatCost, th: &Theme) -> AnyElement {
    let est = if r.estimate { "estimate" } else { "" };
    let cells = vec![r.seat_id.clone(), est.to_string(), tokens(r.tokens), money(r.usd)];
    row_of(cells, th).text_size(px(12.5)).text_color(th.fg).into_any_element()
}

/// Seats table, with the estimate note when any seat is on a subscription.
fn seats_section(seats: &[SeatCost], th: &Theme) -> AnyElement {
    let rows = seats.iter().map(|r| seat_row(r, th)).collect();
    let mut items = table(&["Seat", "", "Tokens", "Spend"], rows, th);
    if seats.iter().any(|s| s.estimate) {
        items.push(muted(ESTIMATE, th).into_any_element());
    }
    section("By seat", items)
}

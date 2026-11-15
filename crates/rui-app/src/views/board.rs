//! Task board column.

use super::column;
use crate::store::{self, Store};
use gpui::{Context, Entity, Window, div, prelude::*, px, rgb};
use rui_core::{Task, TaskStatus};

/// Tasks grouped by client, subtasks nested.
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

fn status_color(s: TaskStatus) -> u32 {
    match s {
        TaskStatus::New | TaskStatus::Open => 0x6b6b78,
        TaskStatus::Clarifying | TaskStatus::InProgress => 0x3b6fd4,
        TaskStatus::WaitingAnswer => 0xd49a1f,
        TaskStatus::Split => 0x8a4fd4,
        TaskStatus::Done => 0x2f9e5a,
        TaskStatus::Stuck => 0xd44040,
    }
}

fn pill(s: TaskStatus) -> impl IntoElement {
    div()
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .text_xs()
        .bg(rgb(status_color(s)))
        .text_color(rgb(0xffffff))
        .child(s.as_str())
}

fn task_row(t: &Task, indent: bool) -> impl IntoElement {
    let row = div().flex().items_center().gap_2().py(px(4.)).text_sm();
    let row = if indent { row.pl(px(20.)) } else { row };
    row.child(pill(t.status))
        .child(div().flex_1().child(t.title.clone()))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x8b8b99))
                .child(format!("{}/{}", t.loops_used, t.max_loops)),
        )
}

fn client_block(name: &str, tasks: &[Task], client_id: i64) -> impl IntoElement {
    let mine: Vec<&Task> = tasks.iter().filter(|t| t.client_id == client_id).collect();
    let mut block = div().flex().flex_col().gap_1().child(
        div().text_color(rgb(0xffffff)).pb(px(4.)).child(name.to_string()),
    );
    for top in mine.iter().filter(|t| t.parent_id.is_none()) {
        block = block.child(task_row(top, false));
        for sub in mine.iter().filter(|t| t.parent_id == Some(top.id)) {
            block = block.child(task_row(sub, true));
        }
    }
    block
}

impl Render for BoardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.store.read(cx);
        let blocks: Vec<_> =
            s.clients.iter().map(|c| client_block(&c.name, &s.tasks, c.id)).collect();
        column("Tasks").child(
            div()
                .id("board-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_4()
                .children(blocks),
        )
    }
}

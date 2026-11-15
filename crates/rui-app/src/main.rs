//! RUI desktop app.

mod api;
mod store;
mod text_input;
mod views;

use gpui::{
    App, Application, Bounds, Context, Entity, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, relative, rgb, size,
};
use rui_core::SeatInfo;
use store::Store;
use views::{
    activity::ActivityView, board::BoardView, chat::ChatView, questions::QuestionsView,
    seat::seat_label,
};

struct Root {
    store: Entity<Store>,
    chat: Entity<ChatView>,
    board: Entity<BoardView>,
    questions: Entity<QuestionsView>,
    activity: Entity<ActivityView>,
}

impl Root {
    fn new(cx: &mut Context<Self>) -> Self {
        let store = cx.new(Store::new);
        store::observe(&store, cx);
        Self {
            chat: cx.new(|cx| ChatView::new(store.clone(), cx)),
            board: cx.new(|cx| BoardView::new(store.clone(), cx)),
            questions: cx.new(|cx| QuestionsView::new(store.clone(), cx)),
            activity: cx.new(|cx| ActivityView::new(store.clone(), cx)),
            store,
        }
    }
}

/// Green or red dot plus the seat description; empty until the first seat fetch.
fn seat_bar(seat: Option<&SeatInfo>) -> Option<impl IntoElement + use<>> {
    let seat = seat?;
    let color = if seat.logged_in { 0x2f9e5a } else { 0xd44040 };
    Some(
        div()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .text_color(rgb(0xb4b4c2))
            .child(div().size(px(8.)).rounded_full().bg(rgb(color)))
            .child(SharedString::from(seat_label(seat))),
    )
}

impl Root {
    /// Questions on top (fits content, max 40%), activity filling the rest.
    fn right_column(&self) -> impl IntoElement + use<> {
        let questions = div()
            .id("questions-wrap")
            .flex_none()
            .max_h(relative(0.4))
            .overflow_y_scroll()
            .child(self.questions.clone());
        let activity = div().flex_1().min_h_0().child(self.activity.clone());
        div().w(relative(0.35)).h_full().flex().flex_col().child(questions).child(activity)
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.store.read(cx).error.clone();
        let seat = seat_bar(self.store.read(cx).seat.as_ref());
        let banner = error.map(|e| {
            div()
                .px(px(12.))
                .py(px(4.))
                .rounded_md()
                .bg(rgb(0xd44040))
                .text_sm()
                .child(SharedString::from(e))
        });
        let top = div()
            .flex()
            .items_center()
            .gap_4()
            .px(px(16.))
            .py(px(10.))
            .border_b_1()
            .border_color(rgb(0x33333d))
            .child(div().text_lg().child("RUI"))
            .children(seat)
            .children(banner);
        let columns = div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(div().w(relative(0.30)).h_full().child(self.chat.clone()))
            .child(div().w(relative(0.35)).h_full().child(self.board.clone()))
            .child(self.right_column());
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1e24))
            .text_color(rgb(0xe6e6ee))
            .text_size(px(14.))
            .child(top)
            .child(columns)
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        text_input::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("RUI".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(Root::new),
        )
        .unwrap();
        cx.activate(true);
    });
}

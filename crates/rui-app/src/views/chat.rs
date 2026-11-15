//! Chat column.

use super::primary_button;
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{Context, Entity, ScrollHandle, Window, div, prelude::*, px};
use rui_core::ChatRole;

/// Chat messages plus an input box.
pub struct ChatView {
    store: Entity<Store>,
    input: Entity<TextInput>,
    scroll: ScrollHandle,
    last_len: usize,
}

impl ChatView {
    /// Build the chat view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let input = cx.new(|cx| TextInput::new("Message Rui...", cx));
        cx.subscribe(&input, |this, _, _: &Submit, cx| this.send(cx)).detach();
        Self { store, input, scroll: ScrollHandle::new(), last_len: 0 }
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).content();
        if text.trim().is_empty() {
            return;
        }
        self.input.update(cx, |i, cx| i.clear(cx));
        self.store.update(cx, |s, cx| s.send_chat(text, cx));
    }
}

fn bubble(role: ChatRole, text: &str, th: &Theme) -> impl IntoElement {
    let user = role == ChatRole::User;
    let row = div().flex().w_full();
    let row = if user { row.justify_end() } else { row.justify_start() };
    row.child(
        div()
            .max_w(px(320.))
            .px(px(12.))
            .py(px(8.))
            .rounded_lg()
            .text_sm()
            .bg(if user { th.accent_fill } else { th.surface })
            .text_color(if user { th.on_accent } else { th.fg })
            .child(text.to_string()),
    )
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let chat = self.store.read(cx).chat.clone();
        if chat.len() != self.last_len {
            self.last_len = chat.len();
            self.scroll.scroll_to_bottom();
        }
        let messages = div()
            .id("chat-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .gap_2()
            .children(chat.iter().map(|m| bubble(m.role, &m.content, &th)));
        let send = primary_button("send", "Send", cx).on_click(cx.listener(|this, _, _, cx| this.send(cx)));
        div().flex().flex_col().size_full().p(px(14.)).gap_3().child(messages).child(
            div().flex().gap_2().items_center().child(div().flex_1().child(self.input.clone())).child(send),
        )
    }
}

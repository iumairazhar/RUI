//! Hide and Remove for threads: quiet buttons on board cards and in the thread page header, and
//! the "Hidden (N)" list at the bottom of the board with an Unhide button per thread.

use super::teams::can_stop;
use super::{Thread, ThreadsView};
use crate::api::ThreadSummary;
use crate::theme::Theme;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Stateful, div, prelude::*, px};
use rui_core::TaskStatus;

/// Small neutral button, like the card's Stop pill but quiet.
fn quiet_pill(id: String, label: &str, th: &Theme) -> Stateful<Div> {
    let hover = th.fg;
    let pill = div().id(SharedString::from(id)).flex().flex_none().items_center().h(px(22.)).px(px(9.)).rounded(px(6.));
    let pill = pill.text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).cursor_pointer();
    let pill = pill.bg(th.surface).text_color(th.fg_2).border_1().border_color(th.hairline);
    pill.hover(move |s| s.text_color(hover)).child(label.to_string())
}

/// The armed "Click again to remove" pill, in the danger colors.
fn armed_pill(id: String, th: &Theme) -> Stateful<Div> {
    let pill = div().id(SharedString::from(id)).flex().flex_none().items_center().h(px(22.)).px(px(9.)).rounded(px(6.));
    let pill = pill.text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).cursor_pointer();
    pill.bg(th.block.dim).text_color(th.block.fg).border_1().border_color(th.block.rim).child("Click again to remove")
}

impl ThreadsView {
    /// A small "⋯" in the card's top-right corner; clicking it opens Hide, Remove and (while it can
    /// run) Stop. The rest of the card stays a plain "open the thread" target.
    pub(super) fn card_actions(&self, id: i64, status: &TaskStatus, group: SharedString, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.card_menu == Some(id);
        let dots = self.menu_button(id, open, th, cx);
        let dots = if open { dots } else { dots.opacity(0.45).group_hover(group, |s| s.opacity(1.)) };
        let corner = div().absolute().right(px(8.)).top(px(8.)).flex().flex_col().items_end().gap(px(4.)).child(dots);
        if !open {
            return corner.into_any_element();
        }
        corner.child(self.card_menu_list(id, status, th, cx)).into_any_element()
    }

    /// The "⋯" toggle; it never opens the thread underneath.
    fn menu_button(&self, id: i64, open: bool, th: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let (bg, hover) = (if open { th.surface_2 } else { th.surface }, th.surface_2);
        let b = div().id(SharedString::from(format!("thread-menu-{id}"))).flex().items_center().justify_center();
        let b = b.size(px(24.)).rounded(px(6.)).bg(bg).border_1().border_color(th.hairline).cursor_pointer();
        let b = b.text_size(px(14.)).text_color(th.fg_2).hover(move |s| s.bg(hover)).child("\u{22ef}");
        b.on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.card_menu = if this.card_menu == Some(id) { None } else { Some(id) };
            this.remove_armed = None;
            cx.notify();
        }))
    }

    /// The open menu: Hide, Remove (two clicks) and Stop, stacked under the "⋯".
    fn card_menu_list(&self, id: i64, status: &TaskStatus, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let hide = quiet_pill(format!("thread-hide-{id}"), "Hide", th).on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.card_menu = None;
            this.hide(id, cx);
        }));
        let stop = can_stop(status).then(|| self.stop_pill(format!("thread-stop-{id}"), id, th, cx));
        let list = div().flex().flex_col().items_end().gap(px(4.)).p(px(6.)).rounded(px(8.)).bg(th.surface);
        let list = list.border_1().border_color(th.hairline).shadow(th.card_shadow());
        list.child(hide).child(self.remove_pill(format!("thread-remove-{id}"), id, cx)).children(stop).into_any_element()
    }

    /// "Remove" first; once clicked, "Click again to remove" drops and hides the thread.
    fn remove_pill(&self, key: String, id: i64, cx: &mut Context<Self>) -> Stateful<Div> {
        let th = crate::theme::theme(cx).clone();
        if self.remove_armed != Some(id) {
            return quiet_pill(key, "Remove", &th).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.remove_armed = Some(id);
                cx.notify();
            }));
        }
        armed_pill(key, &th).on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.remove(id, cx);
        }))
    }

    /// Hide and Remove in a top-level thread's page header.
    pub(super) fn hide_controls(&self, t: &Thread, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if t.task.parent_id.is_some() {
            return vec![];
        }
        let id = t.task.id;
        let hide = quiet_pill("task-hide".into(), "Hide", th).on_click(cx.listener(move |this, _, _, cx| this.hide(id, cx)));
        vec![hide.into_any_element(), self.remove_pill("task-remove".into(), id, cx).into_any_element()]
    }

    /// Hide a thread and go back to the board if its page was open.
    fn hide(&mut self, id: i64, cx: &mut Context<Self>) {
        self.leave_page(id, cx);
        self.store.update(cx, |s, cx| s.hide_thread(id, cx));
    }

    /// Drop (if unfinished) and hide a thread for good.
    fn remove(&mut self, id: i64, cx: &mut Context<Self>) {
        self.remove_armed = None;
        self.leave_page(id, cx);
        self.store.update(cx, |s, cx| s.remove_thread(id, cx));
    }

    fn leave_page(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.open == Some(id) {
            self.close(cx);
        }
        cx.notify();
    }

    /// "Hidden (N)" at the bottom of the board; when open, each hidden thread with an Unhide button.
    pub(super) fn hidden_section(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hidden = self.store.read(cx).hidden_threads.clone();
        if hidden.is_empty() {
            return None;
        }
        let text = format!("Hidden ({})", hidden.len());
        let link = self.toggle_link("hidden-toggle", text, th, cx, |v| v.show_hidden = !v.show_hidden);
        let rows: Vec<AnyElement> = if self.show_hidden { hidden.iter().map(|t| self.hidden_row(t, th, cx)).collect() } else { vec![] };
        let list = div().flex().flex_col().gap(px(6.)).children(rows);
        Some(div().id("hidden-threads").flex().flex_col().gap(px(6.)).mr(px(40.)).pb(px(28.)).child(link).child(list).into_any_element())
    }

    fn hidden_row(&self, t: &ThreadSummary, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = t.id;
        let title = div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(th.fg_2).child(t.title.clone());
        let client = div().flex_none().text_size(px(11.5)).text_color(th.muted).child(t.client.name.clone());
        let unhide = quiet_pill(format!("thread-unhide-{id}"), "Unhide", th);
        let unhide = unhide.on_click(cx.listener(move |this, _, _, cx| this.store.update(cx, |s, cx| s.unhide_thread(id, cx))));
        let row = div().id(("hidden-row", id as usize)).flex().items_center().gap(px(10.)).px(px(10.)).py(px(7.));
        row.rounded(px(10.)).bg(th.surface).child(title).child(client).child(unhide).into_any_element()
    }
}

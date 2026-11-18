//! Client drawer: what the owner knows about one client, and a box to add more.

use super::client_triggers::TriggerPanel;
use super::library::{self as lib, after_slide, run, slide_overlay, toast};
use super::{markdown, primary_button};
use crate::api;
use crate::store::Store;
use crate::text_editor::{Save, TextEditor};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Entity, FontWeight, Render, Window, div, prelude::*, px};

const TITLE: &str = "What you know about this client";
const SUBTITLE: &str = "Always wins over anything the agents inferred.";
const EMPTY: &str = "Nothing yet. Add names, preferences, do's and don'ts.";

/// The `text` field of a `{text}` reply, or empty.
fn reply_text(v: &serde_json::Value) -> String {
    v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string()
}

/// The knowledge to show, or `None` when there is nothing written yet.
fn shown(text: &str) -> Option<&str> {
    let t = text.trim();
    (!t.is_empty()).then_some(t)
}

/// "Added to Acme's knowledge".
fn added_title(client: &str) -> String {
    format!("Added to {client}'s knowledge")
}

/// Drawer over the Clients page for the open client.
pub struct ClientDrawer {
    store: Entity<Store>,
    /// (id, name) of the open client.
    open: Option<(i64, String)>,
    closing: bool,
    /// Current knowledge; `None` while loading.
    text: Option<String>,
    editor: Entity<TextEditor>,
    busy: bool,
    triggers: Entity<TriggerPanel>,
}

impl ClientDrawer {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| TextEditor::new("", cx));
        cx.subscribe(&editor, |this, _, _: &Save, cx| this.add(cx)).detach();
        let triggers = cx.new(|cx| TriggerPanel::new(store.clone(), cx));
        Self { store, open: None, closing: false, text: None, editor, busy: false, triggers }
    }

    /// Open the drawer for a client and load its knowledge.
    pub fn open(&mut self, id: i64, name: String, cx: &mut Context<Self>) {
        (self.open, self.closing, self.text, self.busy) = (Some((id, name)), false, None, false);
        self.editor.update(cx, |e, cx| e.set_text("", cx));
        self.triggers.update(cx, |t, cx| t.open(id, cx));
        run(cx, move || api::get_knowledge(id), move |v, res, cx| v.loaded(id, res, cx));
        cx.notify();
    }

    fn loaded(&mut self, id: i64, res: Result<serde_json::Value, String>, cx: &mut Context<Self>) {
        if self.open.as_ref().map(|o| o.0) != Some(id) {
            return;
        }
        self.text = Some(res.map(|v| reply_text(&v)).unwrap_or_default());
        cx.notify();
    }

    /// Post the editor's text, then show the new knowledge.
    fn add(&mut self, cx: &mut Context<Self>) {
        let note = self.editor.read(cx).text().trim().to_string();
        let Some((id, name)) = self.open.clone() else { return };
        if note.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        run(cx, move || api::add_knowledge(id, &note), move |v, res, cx| v.added(id, &name, res, cx));
        cx.notify();
    }

    fn added(&mut self, id: i64, name: &str, res: Result<serde_json::Value, String>, cx: &mut Context<Self>) {
        self.busy = false;
        match res {
            Ok(val) if self.open.as_ref().map(|o| o.0) == Some(id) => {
                self.text = Some(reply_text(&val));
                self.editor.update(cx, |e, cx| e.set_text("", cx));
                toast(&self.store, &added_title(name), "", cx);
            }
            Ok(_) => toast(&self.store, &added_title(name), "", cx),
            Err(e) => toast(&self.store, "Could not add it", &e, cx),
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open.is_none() || self.closing {
            return;
        }
        self.closing = true;
        cx.notify();
        after_slide(cx, Self::finish_close);
    }

    fn finish_close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            (self.closing, self.open, self.text) = (false, None, None);
            cx.notify();
        }
    }

    fn head(&self, name: &str, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let eyebrow = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("CLIENT");
        let title = div().text_size(px(23.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(name.to_string());
        let left = div().flex().flex_col().gap(px(2.)).min_w_0().child(eyebrow).child(title);
        let close = lib::close_button("client-drawer-close", Self::close, th, cx);
        let top = div().flex().items_start().justify_between().gap(px(12.)).child(left).child(close);
        div().flex_none().px(px(20.)).pt(px(20.)).child(top).into_any_element()
    }

    /// The current knowledge as markdown, the empty line, or a loading note.
    fn current(&self, th: &Theme) -> AnyElement {
        let body = match self.text.as_deref() {
            None => lib::note("Loading\u{2026}", th).into_any_element(),
            Some(t) => match shown(t) {
                Some(md) => markdown::render(md, th.fg_2, 12.5, th),
                None => lib::note(EMPTY, th).into_any_element(),
            },
        };
        div().p(px(16.)).rounded(px(10.)).bg(th.well).child(body).into_any_element()
    }

    fn input(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let boxed = div().id("knowledge-box").h(px(120.)).rounded(px(10.)).overflow_hidden().border_1().border_color(th.hairline);
        let label = if self.busy { "Adding\u{2026}" } else { "Add" };
        let btn = primary_button("knowledge-add", label, cx).on_click(cx.listener(|this, _, _, cx| this.add(cx)));
        let row = div().flex().items_center().gap(px(10.)).child(btn).child(lib::note("Or press \u{2318}S", th));
        div().flex().flex_col().gap(px(10.)).child(boxed.child(self.editor.clone())).child(row).into_any_element()
    }

    fn drawer(&self, name: &str, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let title = div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(TITLE);
        let head = div().flex().flex_col().gap(px(2.)).child(title).child(lib::note(SUBTITLE, th));
        let known = div().flex().flex_col().gap(px(12.)).child(head).child(self.current(th)).child(self.input(th, cx));
        let known = known.into_any_element();
        let auto = self.triggers.clone().into_any_element();
        let body = lib::scroll_body("client-drawer-body", vec![known, auto]);
        lib::drawer_shell("client-drawer", th).child(self.head(name, th, cx)).child(body).into_any_element()
    }
}

impl Render for ClientDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((_, name)) = self.open.clone() else { return div().into_any_element() };
        let th = theme(cx).clone();
        let scrim = lib::scrim("client-scrim", Self::close, cx);
        let drawer = self.drawer(&name, &th, cx);
        slide_overlay("client", scrim, drawer, self.closing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_reply_text() {
        assert_eq!(reply_text(&json!({"text": "- a"})), "- a");
        assert_eq!(reply_text(&json!({})), "");
    }

    #[test]
    fn blank_knowledge_is_empty() {
        assert_eq!(shown("  \n"), None);
        assert_eq!(shown("\n- likes tea\n"), Some("- likes tea"));
    }

    #[test]
    fn toast_names_the_client() {
        assert_eq!(added_title("Acme"), "Added to Acme's knowledge");
    }
}

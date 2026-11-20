//! Client drawer: what the owner knows about one client, and a box to add more.


use super::client_deploy::DeployPanel;
use super::client_edit::{Done, EditPanel};
use super::client_triggers::TriggerPanel;
use super::library::{self as lib, after_slide, run, slide_overlay, toast};
use super::clients::{form_buttons, labeled, summary};
use super::preferences::{FULL_NOTE, reply_full};
use super::{chip, markdown, primary_button};
use crate::api;
use crate::store::Store;
use crate::text_editor::{Save, TextEditor};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Entity, FontWeight, Render, Window, div, prelude::*, px};

const EMPTY: &str = "Nothing saved yet. Add names, preferences, do's and don'ts.";

/// The drawer's tabs; one is shown at a time.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Knowledge,
    Automations,
    Deploy,
    Settings,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Knowledge, Tab::Automations, Tab::Deploy, Tab::Settings];

    fn label(self) -> &'static str {
        match self {
            Tab::Knowledge => "Knowledge",
            Tab::Automations => "Automations",
            Tab::Deploy => "Deploy",
            Tab::Settings => "Settings",
        }
    }
}

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
    /// Over 80% of what reaches the agents.
    full: bool,
    editor: Entity<TextEditor>,
    busy: bool,
    tab: Tab,
    adding: bool,
    triggers: Entity<TriggerPanel>,
    deploy: Entity<DeployPanel>,
    edit: Entity<EditPanel>,
}

impl ClientDrawer {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| TextEditor::new("", cx));
        cx.subscribe(&editor, |this, _, _: &Save, cx| this.add(cx)).detach();
        let triggers = cx.new(|cx| TriggerPanel::new(store.clone(), cx));
        let deploy = cx.new(|cx| DeployPanel::new(store.clone(), cx));
        let edit = cx.new(|cx| EditPanel::new(store.clone(), cx));
        cx.subscribe(&edit, |this, _, _: &Done, cx| this.close(cx)).detach();
        Self { store, open: None, closing: false, text: None, full: false, editor, busy: false, tab: Tab::Knowledge, adding: false, triggers, deploy, edit }
    }

    /// Open the drawer for a client and load its knowledge.
    pub fn open(&mut self, id: i64, name: String, cx: &mut Context<Self>) {
        let edit_name = name.clone();
        (self.open, self.closing, self.text, self.busy) = (Some((id, name)), false, None, false);
        (self.tab, self.adding) = (Tab::Knowledge, false);
        self.editor.update(cx, |e, cx| e.set_text("", cx));
        self.triggers.update(cx, |t, cx| t.open(id, cx));
        self.deploy.update(cx, |d, cx| d.open(id, cx));
        self.edit.update(cx, |e, cx| e.open(id, edit_name, cx));
        run(cx, move || api::get_knowledge(id), move |v, res, cx| v.loaded(id, res, cx));
        cx.notify();
    }

    fn loaded(&mut self, id: i64, res: Result<serde_json::Value, String>, cx: &mut Context<Self>) {
        if self.open.as_ref().map(|o| o.0) != Some(id) {
            return;
        }
        let v = res.unwrap_or_default();
        (self.text, self.full) = (Some(reply_text(&v)), reply_full(&v));
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
                (self.text, self.full) = (Some(reply_text(&val)), reply_full(&val));
                self.adding = false;
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

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn set_adding(&mut self, on: bool, cx: &mut Context<Self>) {
        self.adding = on;
        if !on {
            self.editor.update(cx, |e, cx| e.set_text("", cx));
        }
        cx.notify();
    }

    /// Name, one-line repo path and the small stats.
    fn head(&self, name: &str, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = self.open.as_ref().map(|o| o.0).unwrap_or_default();
        let (repo, stats) = summary(id, self.store.read(cx));
        let title = div().text_size(px(23.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(name.to_string());
        let path = div().truncate().font_family("Menlo").text_size(px(11.5)).text_color(th.muted).child(repo);
        let left = div().flex().flex_col().gap(px(2.)).min_w_0().child(title).child(path).child(lib::note(&stats, th));
        let close = lib::close_button("client-drawer-close", Self::close, th, cx);
        let top = div().flex().items_start().justify_between().gap(px(12.)).child(left.flex_1()).child(close);
        div().flex_none().px(px(20.)).pt(px(20.)).child(top).into_any_element()
    }

    /// Segmented control: one button per tab, the shown one filled.
    fn tab_bar(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let seg = |t: Tab| {
            let on = t == self.tab;
            let b = div().id(("client-tab", t as usize)).flex_1().flex().justify_center().py(px(6.)).rounded(px(7.));
            let b = b.text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).cursor_pointer().child(t.label());
            let b = if on { b.bg(th.ctl).text_color(th.fg) } else { b.text_color(th.muted) };
            b.on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx))).into_any_element()
        };
        let bar = div().flex().gap(px(2.)).p(px(3.)).rounded(px(10.)).bg(th.well);
        let bar = bar.children(Tab::ALL.map(seg));
        div().flex_none().px(px(20.)).pt(px(14.)).child(bar).into_any_element()
    }

    /// The current knowledge as markdown, the empty line, or a loading note.
    fn current(&self, th: &Theme) -> AnyElement {
        match self.text.as_deref() {
            None => lib::note("Loading\u{2026}", th).into_any_element(),
            Some(t) => match shown(t) {
                Some(md) => {
                    let md = div().p(px(16.)).rounded(px(10.)).bg(th.well).child(markdown::render(md, th.fg_2, 12.5, th));
                    div().flex().flex_col().gap(px(8.)).child(md).children(self.full.then(|| lib::note(FULL_NOTE, th))).into_any_element()
                }
                None => lib::note(EMPTY, th).into_any_element(),
            },
        }
    }

    /// The note form: one bordered editor, Save and Cancel.
    fn input(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let boxed = div().id("knowledge-box").h(px(120.)).rounded(px(10.)).overflow_hidden().border_1().border_color(th.hairline);
        let label = if self.busy { "Saving\u{2026}" } else { "Save" };
        let save = primary_button("knowledge-add", label, cx).on_click(cx.listener(|this, _, _, cx| this.add(cx)));
        let cancel = chip("knowledge-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| this.set_adding(false, cx)));
        let row = form_buttons(save, cancel).child(lib::note("Or press \u{2318}S", th));
        let note = labeled("Note", boxed.child(self.editor.clone()).into_any_element(), th);
        div().flex().flex_col().gap(px(10.)).child(note).child(row).into_any_element()
    }

    fn knowledge(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let more = if self.adding {
            self.input(th, cx)
        } else {
            chip("knowledge-open", "+ Add a note", cx).on_click(cx.listener(|this, _, _, cx| this.set_adding(true, cx))).into_any_element()
        };
        let col = div().flex().flex_col().items_start().gap(px(12.));
        col.child(div().w_full().child(self.current(th))).child(div().w_full().child(more)).into_any_element()
    }

    fn drawer(&self, name: &str, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let section = match self.tab {
            Tab::Knowledge => self.knowledge(th, cx),
            Tab::Automations => self.triggers.clone().into_any_element(),
            Tab::Deploy => self.deploy.clone().into_any_element(),
            Tab::Settings => self.edit.clone().into_any_element(),
        };
        let body = lib::scroll_body("client-drawer-body", vec![section]);
        let shell = lib::drawer_shell("client-drawer", th);
        shell.child(self.head(name, th, cx)).child(self.tab_bar(th, cx)).child(body).into_any_element()
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

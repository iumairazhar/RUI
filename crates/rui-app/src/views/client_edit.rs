//! The Settings tab of the client drawer: edit fields and a "Danger zone" with Delete client. A blank field keeps the current value, which
//! is shown as the field's placeholder (the text input has no way to be filled in).

use crate::views::clients::{field, labeled};
use crate::views::library::{self as lib, danger_button, run, toast};
use crate::views::{chip, primary_button};
use crate::api;
use crate::store::Store;
use crate::text_input::TextInput;
use crate::theme::{Theme, theme};
use gpui::{Context, Div, Entity, EventEmitter, FontWeight, Render, Window, div, prelude::*, px};

const SUBTITLE: &str = "Leave a field blank to keep it. Type - in hosts to remove them all.";
const WARNING: &str = "Stops its running work and deletes its threads and history from RUI. The repo folder on disk is not touched. This can't be undone.";

/// Sent after a save or delete so the drawer can close.
pub struct Done;

/// Allowed hosts from the field: `None` when blank (keep), an empty list for "-" (remove all).
pub fn hosts_of(text: &str) -> Option<Vec<String>> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let parts = t.split(|c: char| c == ',' || c.is_whitespace()).map(str::trim).filter(|h| !h.is_empty() && *h != "-");
    Some(parts.map(String::from).collect())
}

/// The PATCH body with only the fields that were filled in.
pub fn patch_body(name: &str, repo: &str, hosts: &str) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if !name.trim().is_empty() {
        body.insert("name".into(), name.trim().into());
    }
    if !repo.trim().is_empty() {
        body.insert("repo_path".into(), repo.trim().into());
    }
    if let Some(h) = hosts_of(hosts) {
        body.insert("hosts".into(), h.into());
    }
    body.into()
}

/// True when the typed text is exactly the client's name.
pub fn confirm_matches(typed: &str, name: &str) -> bool {
    !name.is_empty() && typed.trim() == name
}

/// The edit and delete sections of the client drawer.
pub struct EditPanel {
    store: Entity<Store>,
    /// (id, name, repo_path) of the open client.
    client: Option<(i64, String, String)>,
    name: Entity<TextInput>,
    repo: Entity<TextInput>,
    hosts: Entity<TextInput>,
    confirm: Entity<TextInput>,
    confirming: bool,
    busy: bool,
}

impl EventEmitter<Done> for EditPanel {}

impl EditPanel {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let input = |p: &str, cx: &mut Context<Self>| cx.new(|cx| TextInput::new(p, cx));
        let (name, repo, hosts) = (input("Name", cx), input("Repo folder", cx), input("Hosts", cx));
        let confirm = input("Type the client's name", cx);
        Self { store, client: None, name, repo, hosts, confirm, confirming: false, busy: false }
    }

    /// Show the form for a client; the current values become the placeholders.
    pub fn open(&mut self, id: i64, name: String, cx: &mut Context<Self>) {
        let repo = self.store.read(cx).clients.iter().find(|c| c.id == id).map(|c| c.repo_path.clone()).unwrap_or_default();
        self.name = cx.new(|cx| TextInput::new(&name, cx));
        self.repo = cx.new(|cx| TextInput::new(&repo, cx));
        self.hosts = cx.new(|cx| TextInput::new("Loading\u{2026}", cx));
        self.confirm.update(cx, |i, cx| i.clear(cx));
        (self.client, self.confirming, self.busy) = (Some((id, name, repo)), false, false);
        run(cx, move || api::get_client(id), move |v, res, cx| v.hosts_loaded(id, res, cx));
        cx.notify();
    }

    fn hosts_loaded(&mut self, id: i64, res: Result<serde_json::Value, String>, cx: &mut Context<Self>) {
        if self.client.as_ref().map(|c| c.0) != Some(id) {
            return;
        }
        let list: Vec<&str> = res.as_ref().ok().and_then(|v| v["hosts"].as_array()).map(|a| a.iter().filter_map(|h| h.as_str()).collect()).unwrap_or_default();
        let shown = if list.is_empty() { "No extra hosts".to_string() } else { list.join(", ") };
        self.hosts = cx.new(|cx| TextInput::new(&shown, cx));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some((id, ..)) = self.client.clone() else { return };
        if self.busy {
            return;
        }
        let text = |i: &Entity<TextInput>| i.read(cx).content();
        let body = patch_body(&text(&self.name), &text(&self.repo), &text(&self.hosts));
        self.busy = true;
        run(cx, move || api::update_client(id, body), |v, res, cx| v.saved(res, cx));
        cx.notify();
    }

    fn saved(&mut self, res: Result<serde_json::Value, String>, cx: &mut Context<Self>) {
        self.busy = false;
        match res {
            Ok(v) => self.finish("Client saved", v["name"].as_str().unwrap_or(""), cx),
            Err(e) => toast(&self.store, "Could not save the client", &e, cx),
        }
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Some((id, name, _)) = self.client.clone() else { return };
        if self.busy || !confirm_matches(&self.confirm.read(cx).content(), &name) {
            return;
        }
        self.busy = true;
        run(cx, move || api::delete_client(id), move |v, res, cx| v.deleted(&name, res, cx));
        cx.notify();
    }

    fn deleted(&mut self, name: &str, res: Result<serde_json::Value, String>, cx: &mut Context<Self>) {
        self.busy = false;
        match res {
            Ok(_) => self.finish("Client deleted", name, cx),
            Err(e) => toast(&self.store, "Could not delete the client", &e, cx),
        }
        cx.notify();
    }

    /// Toast, refresh clients and threads, and let the drawer close.
    fn finish(&mut self, title: &str, chip: &str, cx: &mut Context<Self>) {
        toast(&self.store, title, chip, cx);
        self.store.update(cx, |s, cx| s.refresh(cx));
        self.client = None;
        cx.emit(Done);
    }

    fn set_confirming(&mut self, on: bool, cx: &mut Context<Self>) {
        self.confirming = on;
        self.confirm.update(cx, |i, cx| i.clear(cx));
        cx.notify();
    }

    fn form(&self, th: &Theme, cx: &mut Context<Self>) -> Div {
        let label = if self.busy { "Saving\u{2026}" } else { "Save" };
        let save = primary_button("client-edit-save", label, cx).on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        let rows = [("Name", &self.name), ("Repo folder", &self.repo), ("Allowed web hosts (comma separated)", &self.hosts)];
        let fields: Vec<Div> = rows.map(|(l, i)| labeled(l, field(i.clone(), true, th), th)).into();
        div().flex().flex_col().gap(px(12.)).children(fields).child(div().flex().child(save))
    }

    fn danger_zone(&self, name: &str, th: &Theme, cx: &mut Context<Self>) -> Div {
        let zone = div().flex().flex_col().gap(px(10.)).mt(px(8.)).pt(px(16.)).border_t_1().border_color(th.hairline);
        let zone = zone.child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.block.fg).child("Danger zone"));
        if !self.confirming {
            let ask = danger_button("client-delete", "Delete client", true, th).on_click(cx.listener(|this, _, _, cx| this.set_confirming(true, cx)));
            return zone.child(div().flex().child(ask));
        }
        let ready = confirm_matches(&self.confirm.read(cx).content(), name);
        let mut go = danger_button("client-delete-confirm", "Delete client", ready && !self.busy, th);
        if ready && !self.busy {
            go = go.on_click(cx.listener(|this, _, _, cx| this.delete(cx)));
        }
        let cancel = chip("client-delete-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| this.set_confirming(false, cx)));
        let warn = div().text_size(px(12.5)).text_color(th.fg_2).child(WARNING);
        let ask = labeled(&format!("Type \u{201c}{name}\u{201d} to confirm"), field(self.confirm.clone(), true, th), th);
        zone.child(warn).child(ask).child(div().flex().items_center().gap(px(8.)).child(go).child(cancel))
    }
}

impl Render for EditPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let name = self.client.as_ref().map(|c| c.1.clone()).unwrap_or_default();
        let head = lib::note(SUBTITLE, &th);
        let form = self.form(&th, cx);
        let zone = self.danger_zone(&name, &th, cx);
        div().flex().flex_col().gap(px(12.)).child(head).child(form).child(zone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hosts_split_on_commas_and_whitespace() {
        assert_eq!(hosts_of("a.com, b.com\nc.com"), Some(vec!["a.com".into(), "b.com".into(), "c.com".into()]));
        assert_eq!(hosts_of("  "), None);
        assert_eq!(hosts_of("-"), Some(vec![]));
    }

    #[test]
    fn patch_body_has_only_filled_fields() {
        assert_eq!(patch_body(" ", "", ""), json!({}));
        assert_eq!(patch_body(" New ", "/r", "a.com"), json!({"name": "New", "repo_path": "/r", "hosts": ["a.com"]}));
        assert_eq!(patch_body("", "", "-"), json!({"hosts": []}));
    }

    #[test]
    fn confirm_needs_the_exact_name() {
        assert!(confirm_matches(" Acme ", "Acme"));
        assert!(!confirm_matches("acme", "Acme"));
        assert!(!confirm_matches("", ""));
    }
}

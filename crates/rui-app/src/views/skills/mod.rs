//! Skills: a searchable table of SKILL.md files and a drawer to read, edit, attach and restore them.

mod drawer;
mod table;

use self::table::{Filter, counts, visible};
use super::library::{self as lib, EditState, after_slide, run, slide_overlay, toast};
use super::section::{Sec, section_top};
use super::{chip, primary_button};
use crate::api::{self, SkillInfo};
use crate::store::{self, Store};
use crate::text_input::{Submit, TextInput};
use crate::theme::{Theme, theme};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, PathPromptOptions, Stateful, Window, deferred, div, prelude::*, px,
};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Tab of the skill drawer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Review,
    Overview,
    Edit,
    Attach,
    Tools,
    History,
}

/// Which inline form under the toolbar is open.
#[derive(Clone, Copy, PartialEq)]
enum AddMode {
    Write,
    Import,
}

/// A question asked before something destructive.
#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Delete,
    Restore(u64),
}

/// Skills page: toolbar, table and the drawer of the open skill.
pub struct SkillsView {
    store: Entity<Store>,
    search: Entity<TextInput>,
    add_input: Entity<TextInput>,
    filter: Filter,
    menu: bool,
    add: Option<AddMode>,
    add_error: Option<String>,
    open: Option<String>,
    /// Drawer is playing its slide-out before it closes.
    closing: bool,
    tab: Tab,
    detail: Option<SkillInfo>,
    edit: EditState,
    sel_roles: Vec<String>,
    sel_clients: Vec<String>,
    confirm: Option<Confirm>,
    /// Files of the open skill; None while loading.
    files: Option<Vec<api::SkillFile>>,
    open_files: HashSet<String>,
}

impl SkillsView {
    /// Build the skills view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let search = cx.new(|cx| TextInput::new("Search skills", cx));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let add_input = cx.new(|cx| TextInput::new("", cx));
        cx.subscribe(&add_input, |this, _, _: &Submit, cx| this.submit_add(cx)).detach();
        let edit = EditState::new("SKILL.md", Self::save, cx);
        Self {
            store,
            search,
            add_input,
            filter: Filter::All,
            menu: false,
            add: None,
            add_error: None,
            open: None,
            closing: false,
            tab: Tab::Overview,
            detail: None,
            edit,
            sel_roles: vec![],
            sel_clients: vec![],
            confirm: None,
            files: None,
            open_files: HashSet::new(),
        }
    }

    // ---- opening and closing ----

    fn open_skill(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(info) = self.store.read(cx).skills.iter().find(|s| s.name == name).cloned() else { return };
        self.open_with(info, Tab::Review, cx);
        self.fetch(true, cx);
    }

    fn open_with(&mut self, info: SkillInfo, tab: Tab, cx: &mut Context<Self>) {
        self.edit.load(&info.text, cx);
        self.open = Some(info.name.clone());
        (self.closing, self.tab, self.confirm) = (false, tab, None);
        self.detail = Some(info);
        self.reset_selection();
        self.fetch_files(cx);
        cx.notify();
    }

    fn fetch_files(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.open.clone() else { return };
        (self.files, self.open_files) = (None, HashSet::new());
        let want = name.clone();
        run(cx, move || api::skill_files(&name), move |v, res, cx| {
            if v.open.as_deref() != Some(want.as_str()) {
                return;
            }
            v.files = res.ok().and_then(|val| serde_json::from_value(val).ok()).or(Some(vec![]));
            cx.notify();
        });
    }

    fn toggle_file(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.open_files.remove(&path) {
            self.open_files.insert(path);
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
        if !self.closing {
            return;
        }
        (self.closing, self.open, self.detail, self.confirm) = (false, None, None, None);
        (self.files, self.open_files) = (None, HashSet::new());
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.edit.preview = false;
        cx.notify();
    }

    fn reset_selection(&mut self) {
        let Some(d) = &self.detail else { return };
        (self.sel_roles, self.sel_clients) = (d.attach.roles.clone(), d.attach.clients.clone());
    }

    // ---- talking to the daemon ----

    fn path(&self, tail: &str) -> Option<String> {
        self.open.as_ref().map(|n| format!("/skills/{n}{tail}"))
    }

    /// Load the open skill; `reload` also puts its text in the editor.
    fn fetch(&mut self, reload: bool, cx: &mut Context<Self>) {
        let Some(path) = self.path("") else { return };
        run(cx, move || api::library("GET", &path, None), move |v, res, cx| match res {
            Ok(val) => v.apply(val, reload, cx),
            Err(e) => v.fail(&e, cx),
        });
    }

    /// Take a skill reply as the open skill, and refresh the list behind it.
    fn apply(&mut self, val: Value, reload: bool, cx: &mut Context<Self>) {
        let Ok(info) = serde_json::from_value::<SkillInfo>(val) else { return };
        if self.open.as_deref() != Some(info.name.as_str()) {
            return;
        }
        if reload {
            self.edit.load(&info.text, cx);
        }
        self.detail = Some(info);
        self.store.update(cx, |s, cx| s.refresh_skills(cx));
        cx.notify();
    }

    fn fail(&mut self, e: &str, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.action_error = Some(format!("Request failed: {e}"));
            cx.notify();
        });
    }

    /// Post to the open skill, then show `title` and `chip` in a toast.
    fn post(&mut self, tail: &str, body: Value, title: &'static str, chip: String, cx: &mut Context<Self>) {
        let Some(path) = self.path(tail) else { return };
        run(cx, move || api::library("POST", &path, Some(body)), move |v, res, cx| match res {
            Ok(val) => {
                v.apply(val, false, cx);
                toast(&v.store, title, &chip, cx);
            }
            Err(e) => v.fail(&e, cx),
        });
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path("") else { return };
        let why = self.edit.reason(cx);
        if why.is_empty() {
            self.edit.why_missing = true;
            cx.notify();
            return;
        }
        (self.edit.why_missing, self.edit.error) = (false, None);
        let body = json!({ "text": self.edit.text(cx), "why": why });
        run(cx, move || api::library("PUT", &path, Some(body)), |v, res, cx| match res {
            Ok(val) => {
                v.edit.saved(cx);
                v.apply(val, false, cx);
                toast(&v.store, "Saved", "Agents with this skill pick it up on their next loop", cx);
            }
            Err(e) => {
                v.edit.error = Some(e);
                cx.notify();
            }
        });
    }

    fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        self.edit.preview = !self.edit.preview;
        cx.notify();
    }

    fn discard(&mut self, cx: &mut Context<Self>) {
        self.fetch(true, cx);
        toast(&self.store, "Discarded", "Nothing was saved", cx);
    }

    fn set_status(&mut self, status: &'static str, title: &'static str, chip: &str, cx: &mut Context<Self>) {
        self.post("/status", json!({ "status": status }), title, chip.to_string(), cx);
    }

    fn toggle_role(&mut self, name: String, cx: &mut Context<Self>) {
        self.sel_roles = lib::toggled(&self.sel_roles, &name);
        cx.notify();
    }

    fn toggle_client(&mut self, name: String, cx: &mut Context<Self>) {
        self.sel_clients = lib::toggled(&self.sel_clients, &name);
        cx.notify();
    }

    fn apply_attach(&mut self, cx: &mut Context<Self>) {
        let body = json!({ "roles": self.sel_roles, "clients": self.sel_clients });
        self.post("/attach", body, "Attachments saved", "Client skills never leave that client".into(), cx);
    }

    fn toggle_tool(&mut self, tool: String, cx: &mut Context<Self>) {
        let Some(d) = self.detail.as_mut() else { return };
        d.tools = lib::toggled(&d.tools, &tool);
        let body = json!({ "tools": d.tools });
        cx.notify();
        let Some(path) = self.path("/tools") else { return };
        run(cx, move || api::library("POST", &path, Some(body)), |v, res, cx| match res {
            Ok(val) => v.apply(val, false, cx),
            Err(e) => {
                v.fail(&e, cx);
                v.fetch(false, cx);
            }
        });
    }

    // ---- confirm dialog ----

    fn ask_delete(&mut self, cx: &mut Context<Self>) {
        self.confirm = Some(Confirm::Delete);
        cx.notify();
    }

    fn ask_restore(&mut self, ts: u64, cx: &mut Context<Self>) {
        self.confirm = Some(Confirm::Restore(ts));
        cx.notify();
    }

    fn cancel_confirm(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
        cx.notify();
    }

    fn accept_confirm(&mut self, cx: &mut Context<Self>) {
        match self.confirm.take() {
            Some(Confirm::Delete) => self.delete(cx),
            Some(Confirm::Restore(ts)) => self.restore(ts, cx),
            None => {}
        }
        cx.notify();
    }

    fn restore(&mut self, ts: u64, cx: &mut Context<Self>) {
        let Some(path) = self.path("/restore") else { return };
        let when = lib::stamp(ts);
        run(cx, move || api::library("POST", &path, Some(json!({ "ts": ts }))), move |v, res, cx| match res {
            Ok(val) => {
                v.apply(val, true, cx);
                toast(&v.store, "Version restored", &format!("Back to {when}"), cx);
            }
            Err(e) => v.fail(&e, cx),
        });
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(name)) = (self.path(""), self.open.clone()) else { return };
        run(cx, move || api::library("DELETE", &path, None), move |v, res, cx| match res {
            Ok(_) => {
                v.store.update(cx, |s, cx| s.refresh_skills(cx));
                toast(&v.store, "Deleted", &name, cx);
                v.close(cx);
            }
            Err(e) => v.fail(&e, cx),
        });
    }

    // ---- adding skills ----

    fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = !self.menu;
        cx.notify();
    }

    fn start_add(&mut self, mode: AddMode, cx: &mut Context<Self>) {
        (self.menu, self.add, self.add_error) = (false, Some(mode), None);
        self.add_input.update(cx, |i, cx| i.clear(cx));
        cx.notify();
    }

    fn cancel_add(&mut self, cx: &mut Context<Self>) {
        (self.add, self.add_error) = (None, None);
        cx.notify();
    }

    fn submit_add(&mut self, cx: &mut Context<Self>) {
        let text = self.add_input.read(cx).content().trim().to_string();
        match (self.add, text.is_empty()) {
            (Some(AddMode::Write), false) => self.create(text, cx),
            (Some(AddMode::Import), false) => self.import(text, cx),
            _ => {}
        }
    }

    fn create(&mut self, name: String, cx: &mut Context<Self>) {
        run(cx, move || api::library("POST", "/skills", Some(json!({ "name": name }))), |v, res, cx| match res {
            Ok(val) => v.created(val, cx),
            Err(e) => {
                v.add_error = Some(e);
                cx.notify();
            }
        });
    }

    /// A new blank skill: close the form and open its Edit tab.
    fn created(&mut self, val: Value, cx: &mut Context<Self>) {
        self.cancel_add(cx);
        self.store.update(cx, |s, cx| s.refresh_skills(cx));
        if let Ok(info) = serde_json::from_value::<SkillInfo>(val) {
            self.open_with(info, Tab::Edit, cx);
        }
    }

    fn import(&mut self, url: String, cx: &mut Context<Self>) {
        run(cx, move || api::library("POST", "/skills/import", Some(json!({ "url": url }))), |v, res, cx| match res {
            Ok(_) => v.landed("Imported", cx),
            Err(e) => {
                v.add_error = Some(e);
                cx.notify();
            }
        });
    }

    fn landed(&mut self, title: &'static str, cx: &mut Context<Self>) {
        self.cancel_add(cx);
        self.store.update(cx, |s, cx| s.refresh_skills(cx));
        toast(&self.store, title, "It lands off until you review it", cx);
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        self.menu = false;
        cx.notify();
        let options = PathPromptOptions { files: false, directories: true, multiple: false, prompt: None };
        let picked = cx.prompt_for_paths(options);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.first().map(|p| p.to_string_lossy().to_string()) else { return };
            this.update(cx, |v, cx| v.upload(path, cx)).ok();
        })
        .detach();
    }

    fn upload(&mut self, path: String, cx: &mut Context<Self>) {
        run(cx, move || api::library("POST", "/skills/upload", Some(json!({ "path": path }))), |v, res, cx| match res {
            Ok(_) => v.landed("Uploaded", cx),
            Err(e) => v.fail(&e, cx),
        });
    }
}

// ---- rendering ----

/// One segment of the status filter (mock `.seg`): green when on, plain text otherwise.
fn filter_chip(i: usize, f: Filter, n: usize, on: bool, th: &Theme, cx: &mut Context<SkillsView>) -> AnyElement {
    let (bg, fg) = if on { (th.accent_fill, th.on_accent) } else { (gpui::transparent_black(), th.fg_2) };
    let hover = th.surface_2;
    let el = div().id(("skills-filter", i)).flex().items_center().gap(px(5.)).h(px(24.)).px(px(10.)).rounded(px(6.));
    let el = el.bg(bg).text_color(fg).text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).cursor_pointer();
    let el = if on { el } else { el.hover(move |s| s.bg(hover)) };
    let count = div().font_family("Menlo").text_size(px(10.5)).opacity(0.7).child(n.to_string());
    el.on_click(cx.listener(move |this, _, _, cx| {
        this.filter = f;
        cx.notify();
    }))
    .child(f.label())
    .child(count)
    .into_any_element()
}

fn menu_row(id: &'static str, title: &str, desc: &str, th: &Theme) -> Stateful<Div> {
    let hover = th.surface_2;
    let t = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(title.to_string());
    let d = div().text_size(px(11.5)).text_color(th.muted).child(desc.to_string());
    let row = div().id(id).flex().flex_col().gap(px(2.)).px(px(11.)).py(px(9.)).rounded(px(8.));
    row.cursor_pointer().hover(move |s| s.bg(hover)).child(t).child(d)
}

impl SkillsView {
    fn add_menu(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let write = menu_row("add-write", "Write a skill", "Start from a blank SKILL.md", th);
        let upload = menu_row("add-upload", "Upload a folder", "Add a skill folder from your computer", th);
        let import = menu_row("add-import", "Import from repo", "Paste a GitHub link", th);
        let write = write.on_click(cx.listener(|this, _, _, cx| this.start_add(AddMode::Write, cx)));
        let upload = upload.on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)));
        let import = import.on_click(cx.listener(|this, _, _, cx| this.start_add(AddMode::Import, cx)));
        let menu = div().id("skills-add-menu").occlude().absolute().top(px(38.)).right_0().w(px(268.));
        let menu = menu.flex().flex_col().p(px(4.)).rounded(px(12.)).bg(th.surface).border_1().border_color(th.glass_rim);
        deferred(menu.shadow(th.card_shadow()).child(write).child(upload).child(import)).with_priority(2).into_any_element()
    }

    fn toolbar(&self, skills: &[SkillInfo], th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let field = div().w(px(300.)).px(px(10.)).rounded(px(8.)).bg(th.surface).border_1().border_color(th.hairline);
        let field = field.child(self.search.clone());
        let n = counts(skills);
        let chips: Vec<AnyElement> =
            Filter::ALL.iter().enumerate().map(|(i, f)| filter_chip(i, *f, n[i], self.filter == *f, th, cx)).collect();
        let ov = if th.dark { gpui::white() } else { gpui::black() };
        let seg = div().flex().gap(px(2.)).p(px(3.)).rounded(px(8.)).bg(ov.opacity(0.06)).border_1().border_color(th.hairline);
        let add = primary_button("skills-add", "+  Add skill", cx).on_click(cx.listener(|this, _, _, cx| this.toggle_menu(cx)));
        let add = div().relative().child(add).children(self.menu.then(|| self.add_menu(th, cx)));
        let top = div().flex().items_center().gap(px(12.)).child(field).child(div().flex_1()).child(add);
        let bar = div().id("skills-toolbar").flex().flex_col().gap(px(10.)).pb(px(14.));
        bar.child(top).child(div().flex().child(seg.children(chips))).into_any_element()
    }

    fn add_bar(&self, mode: AddMode, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (hint, go) = match mode {
            AddMode::Write => ("Name the skill: lowercase letters, digits and hyphens", "Create"),
            AddMode::Import => ("Paste a GitHub repo, tree or SKILL.md link", "Import"),
        };
        let field = div().flex_1().px(px(12.)).py(px(6.)).rounded(px(9.)).bg(th.surface).shadow(th.card_shadow());
        let field = field.child(self.add_input.clone());
        let go = primary_button("skills-add-go", go, cx).on_click(cx.listener(|this, _, _, cx| this.submit_add(cx)));
        let no = chip("skills-add-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| this.cancel_add(cx)));
        let row = div().flex().items_center().gap(px(8.)).child(field).child(go).child(no);
        let hint = div().text_size(px(11.5)).text_color(th.muted).child(hint);
        let err = self.add_error.as_ref().map(|e| div().text_size(px(12.)).text_color(th.block.fg).child(e.clone()));
        let col = div().id("skills-add-bar").flex().flex_col().gap(px(6.)).pb(px(14.)).max_w(px(640.));
        col.child(row).child(hint).children(err).into_any_element()
    }

    fn empty(&self, any: bool, th: &Theme) -> AnyElement {
        let (title, text) = match any {
            false => ("No skills yet", "Skills teach your agents how you like work done. Add your first one."),
            true => ("No matches", "No skills match your search."),
        };
        let title = div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(title);
        let text = div().text_size(px(12.5)).text_color(th.muted).child(text);
        div().id("skills-empty").flex().flex_col().items_center().gap(px(6.)).py(px(48.)).child(title).child(text).into_any_element()
    }

    fn overlay(&mut self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.detail.clone()?;
        let scrim = lib::scrim("skills-scrim", Self::close, cx);
        let drawer = self.drawer(&d, th, cx);
        Some(slide_overlay("skills", scrim, drawer, self.closing))
    }

    fn confirm_view(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let name = self.open.clone().unwrap_or_default();
        let (title, body, label, danger) = match self.confirm? {
            Confirm::Delete => (format!("Delete {name}?"), "Moves it to the trash and detaches it from every agent.".to_string(), "Delete", true),
            Confirm::Restore(ts) => ("Restore this version?".into(), format!("{} comes back. The current text stays in History.", lib::stamp(ts)), "Restore", false),
        };
        Some(lib::confirm_modal(&title, &body, label, danger, Self::cancel_confirm, Self::accept_confirm, th, cx))
    }
}

impl Render for SkillsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let (skills, roles) = {
            let s = self.store.read(cx);
            (s.skills.clone(), s.roles.clone())
        };
        let query = self.search.read(cx).content();
        let shown = visible(&skills, self.filter, &query);
        let selected = self.open.clone();
        let rows: Vec<AnyElement> =
            shown.iter().enumerate().map(|(i, s)| table::row(i, s, &roles, selected.as_deref() == Some(&s.name), &th, cx)).collect();
        let body = match (shown.is_empty(), skills.is_empty()) {
            (true, none) => self.empty(!none, &th),
            _ => div().id("skills-table").flex().flex_col().child(table::head(&th)).children(rows).into_any_element(),
        };
        let add = self.add.map(|m| self.add_bar(m, &th, cx));
        let page = div().id("skills-scroll").flex().flex_col().size_full().overflow_y_scroll().pr(px(40.)).pb(px(28.));
        let lead = div().text_size(px(13.)).text_color(th.fg_2).child("Teach your agents how to work. Every agent in a pod reads the skills attached to its role.");
        let page = page.child(section_top(Sec::Knowledge, Some(lead.into_any_element()), None, cx)).child(self.toolbar(&skills, &th, cx)).children(add).child(body);
        let (overlay, confirm) = (self.overlay(&th, cx), self.confirm_view(&th, cx));
        div().id("skills-page").relative().size_full().child(page).children(overlay).children(confirm)
    }
}

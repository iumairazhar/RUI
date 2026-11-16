//! Agents: pods in flight and the specialists table, with a drawer to read, edit and tune each one.

mod drawer;
mod form;

use super::activity::{icon, mono};
use super::character::{CharState, character};
use super::library::{self as lib, EditState, after_slide, run, slide_overlay, toast};
use super::roster::{role_code, role_title};
use super::{card, chip, page_head, role_chip};
use crate::api::{self, RoleInfo, SkillInfo};
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, FontWeight, Window, div, prelude::*, px};
use rui_core::{Role, Task, TaskStatus};
use serde_json::{Value, json};

/// Loop limits offered in the drawer.
const LIMITS: [i64; 5] = [3, 4, 6, 8, 10];
const SKILLS_W: f32 = 56.;
const TOOLS_W: f32 = 64.;
const LIMIT_W: f32 = 76.;
const STATUS_W: f32 = 104.;

/// Tab of the agent drawer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Overview,
    Edit,
    Skills,
    Tools,
    Limit,
    History,
}

/// A question asked before something destructive.
#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Delete,
    Restore(u64),
}

/// Agents page.
pub struct AgentsView {
    store: Entity<Store>,
    open: Option<String>,
    /// The "New specialist" form is open in the drawer.
    creating: bool,
    /// Drawer is playing its slide-out before it closes.
    closing: bool,
    tab: Tab,
    detail: Option<RoleInfo>,
    edit: EditState,
    confirm: Option<Confirm>,
    form: form::Form,
}

// ---- pure helpers ----

/// Is a member of this pod with this agent running right now.
fn is_live(pod: &Task, agent: &str, tasks: &[Task], running: &dyn Fn(&Task) -> bool) -> bool {
    let mine = |t: &&Task| (t.id == pod.id || t.parent_id == Some(pod.id)) && t.agent.as_deref() == Some(agent);
    tasks.iter().filter(mine).any(running)
}

/// Lead parents that are not done.
fn pods_in_flight(tasks: &[Task]) -> Vec<&Task> {
    tasks.iter().filter(|t| t.role == Role::Lead && t.parent_id.is_none() && t.status != TaskStatus::Done).collect()
}

/// "2 pods holding 5 agents, ..." counts: (pods, agents).
fn counts(pods: &[&Task]) -> (usize, usize) {
    (pods.len(), pods.iter().map(|p| p.pod.len()).sum())
}

/// Pods in flight that include the role.
fn pods_with<'a>(role: &str, tasks: &'a [Task]) -> Vec<&'a Task> {
    pods_in_flight(tasks).into_iter().filter(|p| p.pod.iter().any(|r| r == role)).collect()
}

/// Role list and client list to post to `/skills/{skill}/attach` so only `role` changes.
fn role_skill_attach(skill: &SkillInfo, role: &str, attach: bool) -> (Vec<String>, Vec<String>) {
    let mut roles: Vec<String> = skill.attach.roles.iter().filter(|r| *r != role).cloned().collect();
    if attach {
        roles.push(role.to_string());
    }
    (roles, skill.attach.clients.clone())
}

/// Skills that are on and not yet attached to the role.
fn attachable<'a>(skills: &'a [SkillInfo], role: &RoleInfo) -> Vec<&'a SkillInfo> {
    skills.iter().filter(|s| s.status == "on" && !role.skills.contains(&s.name)).collect()
}

/// The text under the `## Owns` heading of an AGENT.md, up to the next heading.
fn owns_section(text: &str) -> String {
    let after = text.lines().skip_while(|l| l.trim() != "## Owns").skip(1);
    let lines: Vec<&str> = after.take_while(|l| !l.starts_with("## ")).collect();
    lines.join("\n").trim().to_string()
}

fn heading(text: &str, th: &Theme) -> Div {
    let h = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted);
    h.mt(px(24.)).mb(px(10.)).child(text.to_uppercase())
}

fn owns_checks(th: &Theme) -> Div {
    let lock = icon("lock", 12., th.block.fg);
    let chip = div().flex().items_center().gap(px(5.)).h(px(22.)).px(px(9.)).rounded(px(7.));
    let chip = chip.bg(th.block.dim).border_1().border_color(th.block.rim).text_color(th.block.fg);
    chip.text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).child(lock).child("Owns the checks")
}

impl AgentsView {
    /// Build the agents view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let edit = EditState::new("AGENT.md", Self::save, cx);
        let form = form::Form::new(Self::create, cx);
        Self { store, open: None, creating: false, closing: false, tab: Tab::Overview, detail: None, edit, confirm: None, form }
    }

    // ---- opening and closing ----

    fn open_role(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(info) = self.store.read(cx).roles.iter().find(|r| r.name == name).cloned() else { return };
        self.edit.load(&info.text, cx);
        self.open = Some(info.name.clone());
        (self.creating, self.closing, self.tab, self.confirm) = (false, false, Tab::Overview, None);
        self.detail = Some(info);
        self.fetch(true, cx);
        cx.notify();
    }

    fn start_new(&mut self, cx: &mut Context<Self>) {
        (self.creating, self.closing, self.open, self.detail, self.confirm) = (true, false, None, None, None);
        self.form.reset(cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if (self.open.is_none() && !self.creating) || self.closing {
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
        (self.closing, self.creating, self.open, self.detail, self.confirm) = (false, false, None, None, None);
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.edit.preview = false;
        cx.notify();
    }

    // ---- talking to the daemon ----

    fn path(&self, tail: &str) -> Option<String> {
        self.open.as_ref().map(|n| format!("/roles/{n}{tail}"))
    }

    /// Load the open role; `reload` also puts its text in the editor.
    fn fetch(&mut self, reload: bool, cx: &mut Context<Self>) {
        let Some(path) = self.path("") else { return };
        run(cx, move || api::library("GET", &path, None), move |v, res, cx| match res {
            Ok(val) => v.apply(val, reload, cx),
            Err(e) => v.fail(&e, cx),
        });
    }

    /// Take a role reply as the open role, and refresh the lists behind it.
    fn apply(&mut self, val: Value, reload: bool, cx: &mut Context<Self>) {
        let Ok(info) = serde_json::from_value::<RoleInfo>(val) else { return };
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

    /// Post to the open role, then show `title` and `chip` in a toast.
    fn post(&mut self, tail: &str, body: Value, title: &'static str, chip: &str, cx: &mut Context<Self>) {
        let Some(path) = self.path(tail) else { return };
        let chip = chip.to_string();
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
                toast(&v.store, "Saved", "It picks up the change on its next loop", cx);
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
        self.post("/status", json!({ "status": status }), title, chip, cx);
    }

    fn set_limit(&mut self, n: i64, cx: &mut Context<Self>) {
        self.post("/limit", json!({ "max_loops": n }), "Loop limit set", &format!("Now {n} loops"), cx);
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

    /// Attach or detach one skill: the skill's own role list changes, its clients stay.
    fn set_skill(&mut self, skill: String, attach: bool, cx: &mut Context<Self>) {
        let (Some(role), Some(info)) = (self.open.clone(), self.store.read(cx).skills.iter().find(|s| s.name == skill).cloned()) else { return };
        let (roles, clients) = role_skill_attach(&info, &role, attach);
        let body = json!({ "roles": roles, "clients": clients });
        let path = format!("/skills/{skill}/attach");
        run(cx, move || api::library("POST", &path, Some(body)), |v, res, cx| match res {
            Ok(_) => v.fetch(false, cx),
            Err(e) => v.fail(&e, cx),
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

    // ---- creating ----

    fn create(&mut self, cx: &mut Context<Self>) {
        let body = match self.form.values(cx) {
            Ok(b) => b,
            Err(e) => {
                self.form.error = Some(e);
                cx.notify();
                return;
            }
        };
        self.form.error = None;
        run(cx, move || api::library("POST", "/roles", Some(body)), |v, res, cx| match res {
            Ok(_) => {
                v.store.update(cx, |s, cx| s.refresh_skills(cx));
                toast(&v.store, "Created", "It stays off until you turn it on", cx);
                v.close(cx);
            }
            Err(e) => {
                v.form.error = Some(e);
                cx.notify();
            }
        });
    }
}

// ---- rendering ----

impl AgentsView {
    fn pod_role(&self, pod: &Task, i: usize, agent: &str, color: gpui::Hsla, cx: &Context<Self>) -> AnyElement {
        let th = theme(cx);
        let s = self.store.read(cx);
        let running = |t: &Task| s.live.task(t.id).is_some() || matches!(t.status, TaskStatus::InProgress | TaskStatus::Clarifying);
        let live = is_live(pod, agent, &s.tasks, &running);
        let state: AnyElement = match live {
            true => {
                let id = ("team-rui", pod.id as usize * 32 + i);
                let face = character(CharState::Doing, px(18.), id, color, th);
                div().flex().items_center().gap(px(5.)).child(face).child("In the loop").into_any_element()
            }
            false => div().child("Idle").into_any_element(),
        };
        let name = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(role_title(&s.roles, agent));
        let sub = div().text_size(px(10.5)).text_color(th.muted).child(state);
        let text = div().flex().flex_col().child(name).child(sub);
        let item = div().flex().items_center().gap(px(9.)).px(px(8.)).py(px(8.)).rounded(px(10.)).bg(th.well);
        item.child(role_chip(&role_code(&s.roles, agent), 28., color)).child(text).into_any_element()
    }

    fn pod_card(&self, pod: &Task, cx: &Context<Self>) -> AnyElement {
        let th = theme(cx);
        let s = self.store.read(cx);
        let project = s.clients.iter().find(|c| c.id == pod.client_id).map(|c| c.name.clone()).unwrap_or_default();
        let color = th.client_color(pod.client_id);
        let roles: Vec<AnyElement> = pod.pod.iter().enumerate().map(|(i, a)| self.pod_role(pod, i, a, color, cx)).collect();
        let title = div().text_size(px(14.)).font_weight(FontWeight::BOLD).child(pod.title.clone());
        let proj = div().text_size(px(11.5)).text_color(th.muted).child(project);
        let max = div().ml_auto().text_size(px(11.5)).text_color(th.muted).child(format!("max loops {}", pod.max_loops));
        let head = div().flex().items_center().gap(px(8.)).mb(px(12.)).child(title).child(proj).child(max);
        let roles = div().flex().flex_wrap().gap(px(8.)).children(roles);
        card(cx).id(("pod", pod.id as usize)).p(px(14.)).max_w(px(1040.)).child(head).child(roles).into_any_element()
    }

    fn table_head(&self, th: &Theme) -> Div {
        let cell = |t: &str, w: Option<f32>| {
            let c = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child(t.to_uppercase());
            match w {
                Some(w) => c.flex_none().w(px(w)),
                None => c.flex_1(),
            }
        };
        let row = div().flex().gap(px(12.)).px(px(12.)).pb(px(8.));
        row.child(cell("Specialist", None))
            .child(cell("Skills", Some(SKILLS_W)))
            .child(cell("Tools", Some(TOOLS_W)))
            .child(cell("Loop limit", Some(LIMIT_W)))
            .child(cell("Status", Some(STATUS_W)))
    }

    fn role_row(&self, i: usize, r: &RoleInfo, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let name = r.name.clone();
        let color = th.client_color(1);
        let title = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(r.title.clone());
        let title = div().flex().items_center().gap(px(8.)).child(title).children(r.owns_checks.then(|| owns_checks(&th)));
        let desc = div().text_size(px(11.5)).text_color(th.muted).child(r.description.clone());
        let body = div().flex().flex_col().flex_1().min_w_0().gap(px(3.)).child(title).child(desc);
        let num = |text: String, w: f32| mono(11., th.muted, text).flex_none().w(px(w));
        let selected = self.open.as_deref() == Some(r.name.as_str());
        let hover = th.surface_2;
        let row = div().id(("role-row", i)).flex().items_center().gap(px(12.)).px(px(12.)).py(px(10.));
        let row = row.rounded(px(10.)).bg(if selected { th.surface_2 } else { th.surface }).shadow(th.card_shadow());
        let row = row.cursor_pointer().hover(move |s| s.bg(hover));
        let row = row.on_click(cx.listener(move |this, _, _, cx| this.open_role(&name, cx)));
        let chip = role_chip(&r.code, 28., color);
        let first = div().flex().flex_1().min_w_0().items_center().gap(px(12.)).child(chip).child(body);
        row.child(first)
            .child(num(r.skills.len().to_string(), SKILLS_W))
            .child(num(format!("{} of {}", r.tools.len(), lib::ROLE_TOOLS.len()), TOOLS_W))
            .child(num(r.max_loops.to_string(), LIMIT_W))
            .child(lib::status_cell(&r.status, STATUS_W, &th))
            .into_any_element()
    }

    fn specialists_head(&self, th: &Theme, cx: &mut Context<Self>) -> Div {
        let new = chip("agents-new", "New specialist", cx).on_click(cx.listener(|this, _, _, cx| this.start_new(cx)));
        let title = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("SPECIALISTS");
        div().flex().items_center().justify_between().mt(px(24.)).mb(px(10.)).child(title).child(new)
    }

    fn overlay(&mut self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let scrim = lib::scrim("agents-scrim", Self::close, cx);
        let drawer = match (self.creating, self.detail.clone()) {
            (true, _) => self.new_drawer(th, cx),
            (false, Some(d)) => self.drawer(&d, th, cx),
            (false, None) => return None,
        };
        Some(slide_overlay("agents", scrim, drawer, self.closing))
    }

    fn confirm_view(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let name = self.detail.as_ref().map(|d| d.title.clone()).unwrap_or_default();
        let (title, body, label, danger) = match self.confirm? {
            Confirm::Delete => (format!("Delete {name}?"), "Moves it to the trash. Pods can no longer use it.".to_string(), "Delete", true),
            Confirm::Restore(ts) => ("Restore this version?".into(), format!("{} comes back. The current text stays in History.", lib::stamp(ts)), "Restore", false),
        };
        Some(lib::confirm_modal(&title, &body, label, danger, Self::cancel_confirm, Self::accept_confirm, th, cx))
    }
}

impl Render for AgentsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let (tasks, roles) = {
            let s = self.store.read(cx);
            (s.tasks.clone(), s.roles.clone())
        };
        let pods = pods_in_flight(&tasks);
        let (n, m) = counts(&pods);
        let lead = format!("{n} pods holding {m} agents, each built from the specialists below. A pod takes only the roles its work needs.");
        let lead = div().id("agents-lead").text_size(px(14.)).text_color(th.fg_2).child(lead);
        let cards: Vec<AnyElement> = pods.iter().map(|p| self.pod_card(p, cx)).collect();
        let rows: Vec<AnyElement> = roles.iter().enumerate().map(|(i, r)| self.role_row(i, r, cx)).collect();
        let note = "No agent may edit the checks that judge its own output.";
        let note = div().mt(px(10.)).text_size(px(11.5)).text_color(th.muted).child(note);
        let page = div().id("agents-scroll").flex().flex_col().size_full().overflow_y_scroll().pr(px(40.)).pb(px(28.));
        let page = page.child(page_head("TEAM", "Agents", cx)).child(lead).child(heading("Pods in flight", &th));
        let page = page.child(div().flex().flex_col().gap(px(8.)).children(cards)).child(self.specialists_head(&th, cx));
        let table = div().flex().flex_col().gap(px(6.)).child(self.table_head(&th)).children(rows);
        let page = page.child(table).child(note);
        let (overlay, confirm) = (self.overlay(&th, cx), self.confirm_view(&th, cx));
        div().id("agents-page").relative().size_full().child(page).children(overlay).children(confirm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: i64, parent: Option<i64>, agent: &str, role: &str, status: &str, pod: &[&str]) -> Task {
        serde_json::from_value(json!({"id":id,"client_id":1,"parent_id":parent,"title":"t","body":"",
            "status":status,"done_command":null,"max_loops":5,"loops_used":0,"role":role,
            "branch":null,"source_kind":"user","source_ref":null,"depends_on":[],"agent":agent,
            "pod":pod,"created_at":"a","updated_at":"b"}))
        .unwrap()
    }

    fn skill(name: &str, status: &str, roles: &[&str], clients: &[&str]) -> SkillInfo {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        SkillInfo { name: name.into(), status: status.into(), attach: api::Attach { roles: own(roles), clients: own(clients) }, ..Default::default() }
    }

    #[test]
    fn live_members_and_pods() {
        let pod = ["lead", "backend"];
        let tasks = vec![
            task(1, None, "lead", "lead", "split", &pod),
            task(2, Some(1), "backend", "builder", "in_progress", &pod),
            task(3, Some(1), "tester", "builder", "open", &pod),
            task(4, None, "lead", "lead", "done", &pod),
        ];
        let running = |t: &Task| t.status == TaskStatus::InProgress;
        assert!(is_live(&tasks[0], "backend", &tasks, &running));
        assert!(!is_live(&tasks[0], "tester", &tasks, &running));
        let pods = pods_in_flight(&tasks);
        assert_eq!(pods.len(), 1);
        assert_eq!(counts(&pods), (1, 2));
        assert_eq!(pods_with("backend", &tasks).len(), 1);
        assert!(pods_with("tester", &tasks).is_empty());
    }

    #[test]
    fn attaching_changes_only_the_role_list() {
        let s = skill("alpha", "on", &["lead", "backend"], &["acme"]);
        assert_eq!(role_skill_attach(&s, "backend", false), (vec!["lead".to_string()], vec!["acme".to_string()]));
        assert_eq!(role_skill_attach(&s, "tester", true).0, ["lead", "backend", "tester"]);
        assert_eq!(role_skill_attach(&s, "lead", true).0, ["backend", "lead"]);
        assert!(role_skill_attach(&skill("b", "on", &[], &[]), "x", false).0.is_empty());
    }

    #[test]
    fn picker_lists_on_skills_not_yet_attached() {
        let role = RoleInfo { name: "backend".into(), skills: vec!["alpha".into()], ..Default::default() };
        let all = [skill("alpha", "on", &[], &[]), skill("beta", "on", &[], &[]), skill("gamma", "off", &[], &[]), skill("delta", "review", &[], &[])];
        let names: Vec<&str> = attachable(&all, &role).iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["beta"]);
    }

    #[test]
    fn owns_section_stops_at_the_next_heading() {
        let text = "---\nname: x\n---\n# X\n\n## Job\nDo it\n\n## Owns\n- the api\n- the db\n\n## Never\nlie\n";
        assert_eq!(owns_section(text), "- the api\n- the db");
        assert_eq!(owns_section("## Job\nx"), "");
        assert_eq!(owns_section("## Owns\nlast one"), "last one");
    }
}

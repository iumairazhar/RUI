//! Threads board pieces for teams: grouped cards, bands, Stop buttons and the Stop all confirm.

use super::teams::{LANDED_CAP, can_stop, cap_items, collapsed_line, default_expanded, group_col, group_tasks, latest_stamp, stop_label, stop_targets, team_summary};
use super::{Col, Thread, ThreadsView, chevron, leave, loop_dots, loops_of, state_word};
use crate::api::RoleInfo;
use crate::store::Store;
use crate::theme::Theme;
use crate::views::character::{char_state, character};
use crate::views::library::{confirm_modal, danger_button};
use crate::views::roster::pod_codes;
use crate::views::{motion, role_chip, tag, task_status};
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Stateful, div, prelude::*, px};
use rui_core::{Task, TaskStatus};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Where a task's row is in its stop animation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Phase {
    Idle,
    Leaving,
    Gone,
}

/// A top-level task with the threads that work under it.
pub(super) struct Team {
    root: Task,
    /// Pod role codes, lead left out.
    codes: Vec<String>,
    /// The Lead (when it is active itself) and the members.
    rows: Vec<Thread>,
    agents: usize,
    summary: String,
    expanded: bool,
    can_stop: bool,
    stamp: i64,
}

pub(super) enum Entry {
    Solo(Thread),
    Team(Team),
}

impl Entry {
    fn stamp(&self) -> i64 {
        match self {
            Entry::Solo(t) => latest_stamp(&[&t.task]),
            Entry::Team(t) => t.stamp,
        }
    }
}

/// What one band shows: its entries, and (Standing only) the stopped teams kept apart.
#[derive(Default)]
pub(super) struct Band {
    entries: Vec<Entry>,
    stopped: Vec<Entry>,
}

/// Ids of tasks that have a `stopped` event.
pub(super) fn stopped_ids(s: &Store) -> HashSet<i64> {
    s.events.iter().filter(|e| e.kind == "stopped").filter_map(|e| e.task_id).collect()
}

fn find<'a>(tasks: &'a [Task], id: i64) -> Option<&'a Task> {
    tasks.iter().find(|t| t.id == id)
}

fn entry_of(root: &Task, kids: &[&Task], threads: &[Thread], stopped: &HashSet<i64>, roles: &[RoleInfo], folds: &HashMap<i64, bool>) -> Option<Entry> {
    let thread = |id: i64| threads.iter().find(|t| t.task.id == id).cloned();
    if kids.is_empty() {
        return thread(root.id).map(Entry::Solo);
    }
    let all: Vec<&Task> = std::iter::once(root).chain(kids.iter().copied()).collect();
    let lead_on = !matches!(root.status, TaskStatus::Split | TaskStatus::Done);
    let ids = lead_on.then_some(root.id).into_iter().chain(kids.iter().map(|t| t.id));
    let rows: Vec<Thread> = ids.filter_map(thread).collect();
    let members: Vec<&Task> = kids.iter().copied().filter(|t| t.status != TaskStatus::Split).collect();
    Some(Entry::Team(Team {
        root: root.clone(),
        codes: pod_codes(&root.pod, roles),
        rows,
        agents: members.len(),
        summary: team_summary(&members, stopped),
        expanded: folds.get(&root.id).copied().unwrap_or_else(|| default_expanded(&all, stopped)),
        can_stop: all.iter().any(|t| can_stop(&t.status)),
        stamp: latest_stamp(&all),
    }))
}

impl ThreadsView {
    /// Every band's entries: teams grouped under their top-level task, single tasks as rows.
    pub(super) fn bands(&self, threads: &[Thread], stopped: &HashSet<i64>, cx: &Context<Self>) -> HashMap<Col, Band> {
        let s = self.store.read(cx);
        let mut out: HashMap<Col, Band> = [Col::Needs, Col::Standing, Col::Doing, Col::Landed].into_iter().map(|c| (c, Band::default())).collect();
        for g in group_tasks(&s.tasks) {
            let Some(root) = find(&s.tasks, g.root) else { continue };
            let kids: Vec<&Task> = g.members.iter().filter_map(|id| find(&s.tasks, *id)).collect();
            let all: Vec<&Task> = std::iter::once(root).chain(kids.iter().copied()).collect();
            let Some((col, stop)) = group_col(&all, stopped) else { continue };
            let Some(entry) = entry_of(root, &kids, threads, stopped, &s.roles, &self.folds) else { continue };
            let band = out.get_mut(&col).expect("band exists");
            if stop { band.stopped.push(entry) } else { band.entries.push(entry) }
        }
        if let Some(landed) = out.get_mut(&Col::Landed) {
            landed.entries.sort_by_key(|e| std::cmp::Reverse(e.stamp()));
        }
        out
    }

    /// Drop stop animations once the task has really stopped, or after a few seconds.
    pub(super) fn prune_stopping(&mut self, cx: &Context<Self>) {
        let tasks = &self.store.read(cx).tasks;
        let open = |id: &i64| find(tasks, *id).is_some_and(|t| !matches!(t.status, TaskStatus::Stuck | TaskStatus::Done));
        self.stopping.retain(|id, at| at.elapsed() < Duration::from_secs(4) && open(id));
    }

    pub(super) fn phase(&self, id: i64) -> Phase {
        match self.stopping.get(&id) {
            None => Phase::Idle,
            Some(at) if at.elapsed() < Duration::from_millis(super::LEAVE_MS) => Phase::Leaving,
            Some(_) => Phase::Gone,
        }
    }

    fn begin_stopping(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.stopping.extend(ids.into_iter().map(|id| (id, now)));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(super::LEAVE_MS + 40)).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Stop a task (and its team), play the leave animation, and toast the count.
    fn stop_group(&mut self, id: i64, cx: &mut Context<Self>) {
        let targets = stop_targets(&self.store.read(cx).tasks, id);
        self.begin_stopping(targets, cx);
        self.store.update(cx, |s, cx| s.stop_task(id, cx));
    }

    pub(super) fn cancel_stop_all(&mut self, cx: &mut Context<Self>) {
        self.confirm_all = false;
        cx.notify();
    }

    pub(super) fn accept_stop_all(&mut self, cx: &mut Context<Self>) {
        self.confirm_all = false;
        let tasks = self.store.read(cx).tasks.clone();
        let open = |t: &&Task| t.parent_id.is_none() && !matches!(t.status, TaskStatus::Done | TaskStatus::Stuck);
        let ids: Vec<i64> = tasks.iter().filter(open).flat_map(|t| stop_targets(&tasks, t.id)).collect();
        self.begin_stopping(ids, cx);
        self.store.update(cx, |s, cx| s.stop_all(cx));
    }

    pub(super) fn confirm_view(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let body = "Stop every running team? They keep what they've done so far.";
        let modal = confirm_modal("Stop all teams?", body, "Stop all", true, Self::cancel_stop_all, Self::accept_stop_all, th, cx);
        self.confirm_all.then_some(modal)
    }

    /// Page head, with a danger "Stop all" button while anything is running.
    pub(super) fn page_top(&self, th: &Theme, cx: &mut Context<Self>) -> Div {
        let running = self.store.read(cx).tasks.iter().any(|t| can_stop(&t.status));
        let button = running.then(|| {
            let b = danger_button("threads-stop-all", "Stop all", true, th).mb(px(22.));
            b.on_click(cx.listener(|this, _, _, cx| {
                this.confirm_all = true;
                cx.notify();
            }))
        });
        let head = super::page_head("LIVE WORK", "Threads", cx);
        div().flex().items_end().justify_between().pr(px(40.)).child(head).children(button)
    }

    /// Small red button on a row; the label says whether it stops a team or one member.
    pub(super) fn stop_pill(&self, id: String, team: bool, task_id: i64, th: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let pill = div().id(SharedString::from(id)).flex().flex_none().items_center().h(px(22.)).px(px(9.)).rounded(px(6.));
        let pill = pill.text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).cursor_pointer();
        let pill = pill.bg(th.block.dim).text_color(th.block.fg).border_1().border_color(th.block.rim);
        pill.child(stop_label(team)).on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.stop_group(task_id, cx);
        }))
    }

    fn toggle_team(&mut self, id: i64, expanded: bool, cx: &mut Context<Self>) {
        self.folds.insert(id, !expanded);
        cx.notify();
    }

    fn team_head(&self, t: &Team, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (root, expanded) = (t.root.id, t.expanded);
        let text = |s: String| div().flex_1().min_w_0().truncate().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(s);
        let body = match expanded {
            true => self.team_title(t, text(t.root.title.clone()), th),
            false => text(collapsed_line(&t.root.title, t.agents, &t.summary)),
        };
        let stop = t.can_stop.then(|| self.stop_pill(format!("team-stop-{root}"), true, root, th, cx));
        let hover = th.surface_2;
        div()
            .id(("team-head", root as usize))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(10.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_team(root, expanded, cx)))
            .child(chevron(expanded, th))
            .child(body)
            .children(stop)
            .into_any_element()
    }

    /// Title, pod code chips and the Lead's state.
    fn team_title(&self, t: &Team, title: Div, th: &Theme) -> Div {
        let color = th.client_color(t.root.client_id);
        let chips = t.codes.iter().map(|c| role_chip(c, 16., color));
        let word = if t.root.status == TaskStatus::Split { "Leading" } else { state_word(&t.root.status, false) };
        let lead = tag(task_status(&t.root.status, th), word).flex_none();
        div().flex().flex_1().min_w_0().items_center().gap(px(6.)).child(title).children(chips).child(lead)
    }

    /// A compact sub-row of a team: character, name, state word, loop dots and Stop.
    fn member_row(&self, r: &Thread, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let id = r.task.id;
        let color = th.client_color(r.task.client_id);
        let live = self.store.read(cx).live.task(id).cloned();
        let face = character(char_state(&r.task.status, live.as_ref()), px(22.), ("member-rui", id as usize), color, th);
        let name = div().flex_1().min_w_0().truncate().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(r.name.clone());
        let word = div().flex_none().text_size(px(11.)).text_color(th.muted).child(state_word(&r.task.status, r.owner_asked));
        let stop = can_stop(&r.task.status).then(|| self.stop_pill(format!("member-stop-{id}"), false, id, th, cx));
        let hover = th.surface_2;
        let bg = if self.open == Some(id) { th.surface_2 } else { gpui::transparent_black() };
        let row = div().id(("member-row", id as usize)).flex().items_center().gap(px(8.)).pl(px(30.)).pr(px(10.)).py(px(4.)).rounded(px(8.));
        let row = row.bg(bg).cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(move |this, _, _, cx| this.open_thread(id, cx)));
        let row = row.child(face).child(name).child(word).child(loop_dots(loops_of(&r.task), 5., th)).children(stop);
        match self.phase(id) {
            Phase::Idle => row.into_any_element(),
            Phase::Leaving => leave(row.into_any_element(), ("member-out", id as usize)),
            Phase::Gone => div().into_any_element(),
        }
    }

    fn team_card(&self, t: &Team, index: usize, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let root = t.root.id;
        if self.phase(root) == Phase::Gone {
            return div().into_any_element();
        }
        let rows = t.expanded.then(|| {
            let rows: Vec<AnyElement> = t.rows.iter().map(|r| self.member_row(r, th, cx)).collect();
            div().flex().flex_col().pb(px(6.)).children(rows)
        });
        let card = div().id(("team-card", root as usize)).flex().flex_none().flex_col().mb(px(6.)).rounded(px(10.));
        let card = card.bg(th.surface).shadow(th.card_shadow()).child(self.team_head(t, th, cx)).children(rows);
        let card = motion::rise(card, ("team-in", root as usize), index).into_any_element();
        match self.phase(root) {
            Phase::Leaving => leave(card, ("team-out", root as usize)),
            _ => card,
        }
    }

    fn entry(&self, e: &Entry, i: usize, now: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        match e {
            Entry::Solo(t) => self.moving_row(t, i, now, th, cx),
            Entry::Team(t) => self.team_card(t, i, th, cx),
        }
    }

    fn toggle_link(&self, id: &'static str, text: String, th: &Theme, cx: &mut Context<Self>, flip: fn(&mut Self)) -> AnyElement {
        let hover = th.fg_2;
        div()
            .id(id)
            .flex_none()
            .px(px(6.))
            .py(px(6.))
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(th.muted)
            .cursor_pointer()
            .hover(move |s| s.text_color(hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(this);
                cx.notify();
            }))
            .child(text)
            .into_any_element()
    }

    /// Landed shows the newest teams with a "Show more" toggle.
    fn landed_rows(&self, entries: &[Entry], now: i64, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let refs: Vec<&Entry> = entries.iter().collect();
        let (shown, hidden) = cap_items(refs, LANDED_CAP, self.show_landed);
        let mut rows: Vec<AnyElement> = shown.iter().enumerate().map(|(i, e)| self.entry(e, i, now, th, cx)).collect();
        let text = if self.show_landed { "Show fewer".to_string() } else { format!("Show {hidden} more") };
        if hidden > 0 || (self.show_landed && entries.len() > LANDED_CAP) {
            rows.push(self.toggle_link("landed-toggle", text, th, cx, |v| v.show_landed = !v.show_landed));
        }
        rows
    }

    /// The muted Stopped group at the bottom of Standing by, folded by default.
    fn stopped_rows(&self, stopped: &[Entry], now: i64, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if stopped.is_empty() {
            return vec![];
        }
        let text = if self.show_stopped { "Hide stopped".to_string() } else { format!("Show {} stopped", stopped.len()) };
        let mut rows = vec![self.toggle_link("stopped-toggle", text, th, cx, |v| v.show_stopped = !v.show_stopped)];
        if self.show_stopped {
            let list = stopped.iter().enumerate().map(|(i, e)| self.entry(e, i, now, th, cx));
            rows.push(div().flex().flex_col().opacity(0.6).children(list).into_any_element());
        }
        rows
    }

    pub(super) fn column(&self, col: Col, band: &Band, all: &[Thread], now: i64, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (name, empty) = match col {
            Col::Needs => ("Needs you", "Nothing here right now."),
            Col::Standing => ("Standing by", "Nothing here right now."),
            Col::Doing => ("Doing", "Nothing here right now."),
            Col::Landed => ("Landed", "Nothing has landed yet."),
        };
        let mut rows: Vec<AnyElement> = match col {
            Col::Landed => self.landed_rows(&band.entries, now, th, cx),
            _ => band.entries.iter().enumerate().map(|(i, e)| self.entry(e, i, now, th, cx)).collect(),
        };
        let ghosts = self.leaving(col, all, now, th, cx);
        let stopped = if col == Col::Standing { self.stopped_rows(&band.stopped, now, th, cx) } else { vec![] };
        let none = (rows.is_empty() && ghosts.is_empty() && stopped.is_empty()).then(|| div().text_size(px(12.)).text_color(th.muted).child(empty));
        rows.extend(ghosts);
        let list = div().id(("threads-col-list", col as usize)).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().p(px(8.));
        let list = list.children(rows).children(none).children(stopped);
        div()
            .id(("threads-col", col as usize))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .rounded(px(12.))
            .bg(th.well)
            .border_1()
            .border_color(th.hairline)
            .child(super::column_head(name, band.entries.len() + band.stopped.len(), th))
            .child(list)
            .into_any_element()
    }
}

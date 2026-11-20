//! RUI desktop app.

mod api;
mod assets;
mod notify;
mod store;
mod text_editor;
mod text_input;
mod theme;
mod views;

use assets::Assets;
use gpui::AnimationExt;
use gpui::{
    AnyElement, App, Application, Bounds, Context, Entity, FontWeight, PathPromptOptions, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, point, prelude::*, px, size, svg,
};
use rui_core::{Task, TaskStatus};
use store::{Store, StoreHandle};
use theme::{Theme, theme};
use views::motion::{self, spring};
use views::section::{Sec, active_tab, empty_page};
use views::{
    activity::ThreadsView, agents::AgentsView, apps::AppsView, chat::ChatView, client_knowledge::ClientDrawer, clients,
    inbox::InboxPanel,
    library::Tooltip, playbook, results::ResultsView, seat, seat::SeatView, skills::SkillsView,
    spend::{Mode as SpendMode, SpendView},
};

#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Rui,
    Inbox,
    Work,
    Clients,
    Knowledge,
    Cost,
    Analytics,
    Settings,
}

impl Nav {
    const ALL: [Nav; 8] =
        [Nav::Rui, Nav::Inbox, Nav::Work, Nav::Clients, Nav::Knowledge, Nav::Cost, Nav::Analytics, Nav::Settings];

    fn index(self) -> usize {
        Nav::ALL.iter().position(|n| *n == self).unwrap_or(0)
    }

    /// Rail group: dividers sit between groups.
    fn group(self) -> usize {
        match self {
            Nav::Rui | Nav::Inbox | Nav::Work => 0,
            Nav::Settings => 2,
            _ => 1,
        }
    }

    /// Page to open at launch: Work by default, or `RUI_NAV=rui` / `RUI_NAV=clients:1` (section, then sub-tab).
    fn from_env() -> Nav {
        let want = std::env::var("RUI_NAV").unwrap_or_default().to_lowercase();
        let name = want.split(':').next().unwrap_or("");
        Nav::ALL.into_iter().find(|n| n.label().to_lowercase() == name).unwrap_or(Nav::Work)
    }

    /// Sub-tab from `RUI_NAV=<section>:<n>`, keyed like the store's `tabs`.
    fn tab_from_env() -> Option<(&'static str, usize)> {
        let want = std::env::var("RUI_NAV").ok()?;
        let n = want.split(':').nth(1)?.parse().ok()?;
        let key = match Nav::from_env() {
            Nav::Work => Sec::Work.key(),
            Nav::Clients => Sec::Clients.key(),
            Nav::Knowledge => Sec::Knowledge.key(),
            Nav::Cost => Sec::Cost.key(),
            _ => return None,
        };
        Some((key, n))
    }

    fn label(self) -> &'static str {
        match self {
            Nav::Rui => "Rui",
            Nav::Inbox => "Inbox",
            Nav::Work => "Work",
            Nav::Clients => "Clients",
            Nav::Knowledge => "Knowledge",
            Nav::Cost => "Cost",
            Nav::Analytics => "Analytics",
            Nav::Settings => "Settings",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Nav::Rui => "icons/nav-rui.svg",
            Nav::Inbox => "icons/nav-inbox.svg",
            Nav::Work => "icons/threads.svg",
            Nav::Clients => "icons/nav-clients.svg",
            Nav::Knowledge => "icons/nav-knowledge.svg",
            Nav::Cost => "icons/nav-cost.svg",
            Nav::Analytics => "icons/nav-analytics.svg",
            Nav::Settings => "icons/nav-settings.svg",
        }
    }
}

/// Rail selection pill slide, with the spring.
const PILL_MS: u64 = 540;
/// Rail item height plus the gap below it.
const RAIL_STEP: f32 = 47.;
/// Brand tile with its gap, then a divider block (margins plus the hairline).
const RAIL_TILE: f32 = 51.;
const RAIL_DIVIDER: f32 = 17.5;
/// Pages start below the title bar so scrolled content never slides under the header.
const CONTENT_TOP: f32 = 44.;

/// Top of a rail item inside the rail's padding box.
fn rail_y(nav: Nav) -> f32 {
    let dividers = nav.group() + 1;
    RAIL_TILE + RAIL_DIVIDER * dividers as f32 + RAIL_STEP * nav.index() as f32
}

/// "4 clients · 11 threads · 1 seat free".
fn summary_text(clients: usize, threads: usize, free: usize) -> String {
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let parts = [plural(clients, "client", "clients"), plural(threads, "thread", "threads"), plural(free, "seat", "seats")];
    format!("{} \u{b7} {} \u{b7} {} free", parts[0], parts[1], parts[2])
}

/// Top-level tasks that are not done.
fn open_threads(tasks: &[Task]) -> usize {
    tasks.iter().filter(|t| t.parent_id.is_none() && t.status != TaskStatus::Done).count()
}

struct Root {
    store: Entity<Store>,
    nav: Nav,
    /// Item the selection pill is sliding away from.
    pill_from: Nav,
    /// Bumped on every selection so the pill animation restarts.
    pill_seq: usize,
    /// Level the limit notice was dismissed at; a worse level shows it again.
    limit_dismissed: Option<seat::limits::Level>,
    chat: Entity<ChatView>,
    inbox: Entity<InboxPanel>,
    activity: Entity<ThreadsView>,
    agents: Entity<AgentsView>,
    apps: Entity<AppsView>,
    skills: Entity<SkillsView>,
    seats: Entity<SeatView>,
    spend: Entity<SpendView>,
    analytics: Entity<SpendView>,
    results: Entity<ResultsView>,
    /// Drawer of the client opened from the Clients page.
    client_drawer: Entity<ClientDrawer>,
}

impl Root {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.set_global(theme::for_appearance(window.appearance()));
        cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(theme::for_appearance(window.appearance()));
            cx.notify();
        })
        .detach();
        let store = cx.new(Store::new);
        cx.set_global(StoreHandle(store.clone()));
        if let Some((key, n)) = Nav::tab_from_env() {
            store.update(cx, |s, _| s.tabs.insert(key, n));
        }
        cx.observe(&store, |this, store, cx| {
            if store.update(cx, |s, _| std::mem::take(&mut s.open_threads)) {
                store.update(cx, |s, _| s.tabs.insert(Sec::Work.key(), 0));
                this.select(Nav::Work, cx);
            }
            if store.update(cx, |s, _| std::mem::take(&mut s.open_seats)) {
                this.go_to_seats(cx);
            }
            if let Some(id) = store.update(cx, |s, _| s.open_task.take()) {
                let changes = store.update(cx, |s, _| std::mem::take(&mut s.open_changes));
                this.activity.update(cx, |v, cx| if changes { v.show_changes(id, cx) } else { v.open_thread(id, cx) });
            }
            if let Some(id) = store.update(cx, |s, _| s.open_question.take()) {
                this.open_in_inbox(id, cx);
            }
            if let Some(req) = store.update(cx, |s, _| s.result_request.take()) {
                this.results.update(cx, |v, cx| v.open(req, cx));
            }
            cx.notify();
        })
        .detach();
        Self {
            nav: Nav::from_env(),
            pill_from: Nav::from_env(),
            pill_seq: 0,
            limit_dismissed: None,
            chat: cx.new(|cx| ChatView::new(store.clone(), cx)),
            inbox: cx.new(|cx| InboxPanel::new(store.clone(), cx)),
            activity: cx.new(|cx| ThreadsView::new(store.clone(), cx)),
            agents: cx.new(|cx| AgentsView::new(store.clone(), cx)),
            apps: cx.new(|cx| AppsView::new(store.clone(), cx)),
            skills: cx.new(|cx| SkillsView::new(store.clone(), cx)),
            seats: cx.new(|cx| SeatView::new(store.clone(), cx)),
            spend: cx.new(|cx| SpendView::new(store.clone(), SpendMode::Spend, cx)),
            analytics: cx.new(|cx| SpendView::new(store.clone(), SpendMode::Analytics, cx)),
            results: cx.new(|cx| ResultsView::new(store.clone(), cx)),
            client_drawer: cx.new(|cx| ClientDrawer::new(store.clone(), cx)),
            store,
        }
    }

    /// The page for the selected rail section and its sub-tab.
    fn page(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = |sec| active_tab(sec, cx);
        match self.nav {
            Nav::Rui => self.chat.clone().into_any_element(),
            Nav::Inbox => self.inbox.clone().into_any_element(),
            Nav::Work => match tab(Sec::Work) {
                0 => self.activity.clone().into_any_element(),
                _ => self.apps.clone().into_any_element(),
            },
            Nav::Clients => match tab(Sec::Clients) {
                0 => clients::page(self.add_client_button(cx), &self.client_drawer, cx),
                _ => self.agents.clone().into_any_element(),
            },
            Nav::Knowledge => match tab(Sec::Knowledge) {
                0 => self.skills.clone().into_any_element(),
                _ => playbook::page(cx),
            },
            Nav::Cost => match tab(Sec::Cost) {
                0 => self.spend.clone().into_any_element(),
                _ => self.seats.clone().into_any_element(),
            },
            Nav::Analytics => self.analytics.clone().into_any_element(),
            Nav::Settings => empty_page(Sec::Settings, "Settings come later.", cx),
        }
    }

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("content")
            .absolute()
            .top(px(CONTENT_TOP))
            .left_0()
            .right_0()
            .bottom_0()
            .overflow_hidden()
            .pl(px(96.))
            .child(div().id("content-page").size_full().child(self.page(cx)))
            .into_any_element()
    }

    fn add_client_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let btn = views::primary_button("clients-add", "+  Add client", cx);
        btn.on_click(cx.listener(|this, _, _, cx| this.pick_client_folder(cx))).into_any_element()
    }

    /// Pick a repo folder, then create a client named after it.
    fn pick_client_folder(&mut self, cx: &mut Context<Self>) {
        let options = PathPromptOptions { files: false, directories: true, multiple: false, prompt: None };
        let picked = cx.prompt_for_paths(options);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.first().map(|p| p.to_string_lossy().to_string()) else { return };
            this.update(cx, |v, cx| v.add_client(path, cx)).ok();
        })
        .detach();
    }

    fn add_client(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(name) = clients::folder_name(&path) else { return };
        let call_name = name.clone();
        views::library::run(cx, move || api::create_client(&call_name, &path), move |v, res, cx| match res {
            Ok(_) => {
                v.store.update(cx, |s, cx| s.refresh(cx));
                views::library::toast(&v.store, &format!("Added {name}"), "", cx);
            }
            Err(e) => views::library::toast(&v.store, "Could not add client", &e, cx),
        });
    }

    fn summary(&self, cx: &Context<Self>) -> String {
        let s = self.store.read(cx);
        let base = summary_text(s.clients.len(), open_threads(&s.tasks), seat::seats_free(s));
        if self.limit_shown(cx) {
            return base;
        }
        let limit = s.seat.as_ref().and_then(|x| seat::limits::summary(x, store::now_secs(), views::activity::local_offset()));
        limit.map_or(base.clone(), |l| format!("{base} \u{b7} {l}"))
    }

    fn header(&self, th: &Theme, cx: &Context<Self>) -> impl IntoElement + use<> {
        let s = self.store.read(cx);
        let error = s.error.clone().or_else(|| s.action_error.clone());
        let error = error.map(|e| div().id("header-error").text_color(th.block.fg).child(e));
        div()
            .id("header")
            .absolute()
            .left(px(86.))
            .top(px(14.))
            .h(px(22.))
            .flex()
            .items_center()
            .gap(px(9.))
            .child(div().id("header-title").text_size(px(15.)).font_weight(FontWeight::BOLD).child("Rui"))
            .child(div().id("header-summary").text_size(px(12.)).text_color(th.muted).child(self.summary(cx)))
            .children(error.map(|e| e.text_size(px(12.))))
    }

    /// The limit pill's numbers, when it should show.
    fn limit_info(&self, cx: &App) -> Option<seat::limit_ring::RingInfo> {
        let seat = self.store.read(cx).seat.as_ref()?;
        let info = seat::limit_ring::info(seat, store::now_secs(), views::activity::local_offset())?;
        seat::limit_ring::visible(&info, self.limit_dismissed).then_some(info)
    }

    fn limit_shown(&self, cx: &App) -> bool {
        self.limit_info(cx).is_some()
    }

    /// Title-bar pill with the 5-hour limit left; click opens Seats, × hides it.
    fn limit_notice(&self, th: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let info = self.limit_info(cx)?;
        let (row, dismiss) = seat::limit_ring::body(&info, th);
        let level = info.level;
        let row = row.id("limit-ring-open").cursor_pointer().on_click(cx.listener(|v, _, _, cx| v.go_to_seats(cx)));
        let dismiss = dismiss.on_click(cx.listener(move |v, _, _, cx| {
            cx.stop_propagation();
            v.limit_dismissed = Some(level);
            cx.notify();
        }));
        Some(seat::limit_ring::card(row.into_any_element(), dismiss.into_any_element(), th).into_any_element())
    }

    /// Inbox items that are not snoozed.
    fn open_questions(&self, cx: &Context<Self>) -> usize {
        self.store.read(cx).awake_inbox()
    }

    /// Switch to Cost → Seats.
    fn go_to_seats(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, _| s.tabs.insert(Sec::Cost.key(), 1));
        self.select(Nav::Cost, cx);
    }

    /// Switch to the Inbox page with one question's card open.
    fn open_in_inbox(&mut self, qid: i64, cx: &mut Context<Self>) {
        self.select(Nav::Inbox, cx);
        self.inbox.update(cx, |p, cx| p.open_item("question", qid, cx));
    }

    fn badge(&self, th: &Theme, count: usize) -> impl IntoElement + use<> {
        div()
            .id("inbox-badge")
            .absolute()
            .top(px(3.))
            .right(px(3.))
            .min_w(px(16.))
            .h(px(16.))
            .px(px(4.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .bg(th.block.base)
            .text_color(th.badge_text)
            .text_size(px(9.5))
            .font_weight(FontWeight::BOLD)
            .font_family("Menlo")
            .border_2()
            .border_color(th.stage)
            .child(count.to_string())
    }

    fn rail_item(&self, nav: Nav, th: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let active = self.nav == nav;
        let label = nav.label();
        let group = SharedString::from(format!("rail-{label}"));
        let icon_color = if active { th.on_accent } else { th.fg_2 };
        let fg = th.fg;
        let icon = svg().path(nav.icon()).size(px(21.)).text_color(icon_color);
        let icon = if active { icon } else { icon.group_hover(group.clone(), move |s| s.text_color(fg)) };
        let hairline = th.hairline;
        let item = div()
            .id(label)
            .group(group)
            .relative()
            .size(px(44.))
            .mb(px(3.))
            .rounded(px(10.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(icon)
            .on_click(cx.listener(move |this, _, _, cx| this.select(nav, cx)))
            .tooltip(move |_, cx| cx.new(|_| Tooltip(label.into())).into());
        let item = if active { item } else { item.hover(move |s| s.bg(hairline)) };
        let count = if nav == Nav::Inbox { self.open_questions(cx) } else { 0 };
        item.children((count > 0).then(|| self.badge(th, count)))
    }

    fn select(&mut self, nav: Nav, cx: &mut Context<Self>) {
        if nav == self.nav {
            return;
        }
        (self.pill_from, self.nav) = (self.nav, nav);
        self.pill_seq += 1;
        if nav == Nav::Knowledge {
            self.store.update(cx, |s, cx| s.refresh_skills(cx));
        }
        if nav == Nav::Cost {
            self.spend.update(cx, |v, cx| v.load(cx));
        }
        if nav == Nav::Analytics {
            self.analytics.update(cx, |v, cx| v.load(cx));
        }
        if nav == Nav::Work {
            self.activity.update(cx, |v, _| v.reset_motion());
        }
        cx.notify();
    }

    /// Selection pill behind the rail items, springing between them.
    fn pill(&self, th: &Theme) -> impl IntoElement + use<> {
        let (from, to) = (8. + rail_y(self.pill_from), 8. + rail_y(self.nav));
        let pill = div().id("rail-pill").absolute().left(px(8.)).top(px(to)).size(px(44.)).rounded(px(10.)).bg(th.sel);
        pill.with_animation(("rail-pill", self.pill_seq), motion::once(PILL_MS), move |el, d| {
            el.top(px(from + (to - from) * spring(d)))
        })
    }

    /// The brand/filter tile at the top of the rail.
    fn brand_tile(&self, th: &Theme) -> impl IntoElement + use<> {
        let ov = if th.dark { gpui::white() } else { gpui::black() };
        let icon = svg().path("icons/nav-filter.svg").size(px(19.)).text_color(th.fg_2);
        div()
            .id("rail-brand")
            .size(px(44.))
            .mb(px(7.))
            .rounded(px(12.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .bg(ov.opacity(0.10))
            .border_1()
            .border_color(th.glass_rim)
            .child(icon)
            .tooltip(|_, cx| cx.new(|_| Tooltip("All clients".into())).into())
    }

    fn divider(&self, id: usize, th: &Theme) -> impl IntoElement + use<> {
        div().id(("rail-divider", id)).flex_none().w(px(24.)).h(px(0.5)).mt(px(6.)).mb(px(11.)).bg(th.glass_rim)
    }

    fn rail(&self, th: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut items: Vec<AnyElement> = vec![self.brand_tile(th).into_any_element()];
        for (i, nav) in Nav::ALL.iter().enumerate() {
            if i == 0 || Nav::ALL[i - 1].group() != nav.group() {
                items.push(self.divider(nav.group(), th).into_any_element());
            }
            items.push(self.rail_item(*nav, th, cx).into_any_element());
        }
        div()
            .id("rail")
            .absolute()
            .left(px(12.))
            .top(px(54.))
            .bottom(px(12.))
            .w(px(60.))
            .rounded(px(18.))
            .bg(th.glass)
            .border_1()
            .border_color(th.glass_rim)
            .p(px(8.))
            .flex()
            .flex_col()
            .items_center()
            .child(self.pill(th))
            .children(items)
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        div()
            .id("root")
            .relative()
            .size_full()
            .bg(th.stage)
            .font_family(".SystemUIFont")
            .text_color(th.fg)
            .text_size(px(13.))
            .child(self.content(cx))
            .child(self.header(&th, cx))
            .child(self.rail(&th, cx))
            .children(self.limit_notice(&th, cx))
            .children(self.store.read(cx).toast.as_ref().map(|t| views::toast::toast(t, &th)))
            .children(self.results.read(cx).is_open().then(|| self.results.clone()))
    }
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        text_input::bind_keys(cx);
        text_editor::bind_keys(cx);
        cx.set_global(Theme::dark());
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.), px(600.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("RUI".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(21.), px(19.))),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Root::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_positions_follow_the_dividers() {
        assert_eq!(rail_y(Nav::Rui), 68.5);
        assert_eq!(rail_y(Nav::Work), 162.5);
        assert_eq!(rail_y(Nav::Clients), 227.);
        assert_eq!(rail_y(Nav::Settings), 432.5);
    }

    #[test]
    fn summary_reads_like_the_mock() {
        assert_eq!(summary_text(4, 11, 1), "4 clients \u{b7} 11 threads \u{b7} 1 seat free");
        assert_eq!(summary_text(1, 1, 0), "1 client \u{b7} 1 thread \u{b7} 0 seats free");
    }
}

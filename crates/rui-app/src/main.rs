//! RUI desktop app.

mod api;
mod assets;
mod store;
mod text_editor;
mod text_input;
mod theme;
mod views;

use assets::Assets;
use gpui::{
    AnyElement, App, Application, Bounds, Context, Entity, FontWeight, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size, svg,
};
use gpui::AnimationExt;
use views::motion::{self, spring};
use rui_core::QuestionStatus;
use store::Store;
use theme::{Theme, theme};
use views::{
    activity::ThreadsView,
    apps::AppsView,
    board::BoardView,
    chat::ChatView,
    questions::QuestionsView,
    seat::{SeatView, seat_label},
    skills::SkillsView,
    team::TeamView,
};

#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Rui,
    Threads,
    Team,
    Apps,
    Inbox,
    Work,
    Skills,
    Seats,
}

impl Nav {
    const ALL: [Nav; 8] =
        [Nav::Rui, Nav::Threads, Nav::Team, Nav::Apps, Nav::Inbox, Nav::Work, Nav::Skills, Nav::Seats];

    fn index(self) -> usize {
        Nav::ALL.iter().position(|n| *n == self).unwrap_or(0)
    }

    fn label(self) -> &'static str {
        match self {
            Nav::Rui => "Rui",
            Nav::Threads => "Threads",
            Nav::Team => "Team",
            Nav::Apps => "Apps",
            Nav::Inbox => "Inbox",
            Nav::Work => "Work",
            Nav::Skills => "Skills",
            Nav::Seats => "Seats",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Nav::Rui => "icons/rui.svg",
            Nav::Threads => "icons/threads.svg",
            Nav::Team => "icons/team.svg",
            Nav::Apps => "icons/apps.svg",
            Nav::Inbox => "icons/inbox.svg",
            Nav::Work => "icons/work.svg",
            Nav::Skills => "icons/skills.svg",
            Nav::Seats => "icons/seats.svg",
        }
    }
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx);
        div()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(th.ctl)
            .text_size(px(12.))
            .text_color(th.fg)
            .child(self.0.clone())
    }
}

/// Rail selection pill slide, with the spring.
const PILL_MS: u64 = 540;
/// Rail item height plus the gap between items.
const RAIL_STEP: f32 = 47.;

struct Root {
    store: Entity<Store>,
    nav: Nav,
    /// Item the selection pill is sliding away from.
    pill_from: Nav,
    /// Bumped on every selection so the pill animation restarts.
    pill_seq: usize,
    chat: Entity<ChatView>,
    board: Entity<BoardView>,
    questions: Entity<QuestionsView>,
    activity: Entity<ThreadsView>,
    team: Entity<TeamView>,
    apps: Entity<AppsView>,
    skills: Entity<SkillsView>,
    seats: Entity<SeatView>,
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
        cx.observe(&store, |this, store, cx| {
            if store.update(cx, |s, _| std::mem::take(&mut s.open_threads)) {
                this.select(Nav::Threads, cx);
            }
            cx.notify();
        })
        .detach();
        Self {
            nav: Nav::Rui,
            pill_from: Nav::Rui,
            pill_seq: 0,
            chat: cx.new(|cx| ChatView::new(store.clone(), cx)),
            board: cx.new(|cx| BoardView::new(store.clone(), cx)),
            questions: cx.new(|cx| QuestionsView::new(store.clone(), cx)),
            activity: cx.new(|cx| ThreadsView::new(store.clone(), cx)),
            team: cx.new(|cx| TeamView::new(store.clone(), cx)),
            apps: cx.new(|cx| AppsView::new(store.clone(), cx)),
            skills: cx.new(|cx| SkillsView::new(store.clone(), cx)),
            seats: cx.new(|cx| SeatView::new(store.clone(), cx)),
            store,
        }
    }

    fn content(&self) -> AnyElement {
        let page = match self.nav {
            Nav::Rui => self.chat.clone().into_any_element(),
            Nav::Threads => self.activity.clone().into_any_element(),
            Nav::Team => self.team.clone().into_any_element(),
            Nav::Apps => self.apps.clone().into_any_element(),
            Nav::Inbox => self.questions.clone().into_any_element(),
            Nav::Work => self.board.clone().into_any_element(),
            Nav::Skills => self.skills.clone().into_any_element(),
            Nav::Seats => self.seats.clone().into_any_element(),
        };
        div()
            .id("content")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .pl(px(96.))
            .child(div().id("content-page").size_full().child(page))
            .into_any_element()
    }

    fn summary(&self, cx: &Context<Self>) -> String {
        let s = self.store.read(cx);
        let mut text = format!("{} clients · {} tasks", s.clients.len(), s.tasks.len());
        if let Some(seat) = &s.seat {
            text.push_str(" · ");
            text.push_str(&seat_label(seat));
        }
        text
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
            .child(
                div()
                    .id("header-summary")
                    .text_size(px(12.))
                    .text_color(th.muted)
                    .child(self.summary(cx)),
            )
            .children(error.map(|e| e.text_size(px(12.))))
    }

    fn open_questions(&self, cx: &Context<Self>) -> usize {
        let qs = &self.store.read(cx).questions;
        qs.iter().filter(|q| q.status == QuestionStatus::Open).count()
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
            .rounded(px(10.))
            .flex()
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
        if nav == Nav::Skills {
            self.store.update(cx, |s, cx| s.refresh_skills(cx));
        }
        if nav == Nav::Threads {
            self.activity.update(cx, |v, _| v.reset_motion());
        }
        cx.notify();
    }

    /// Selection pill behind the rail items, springing between them.
    fn pill(&self, th: &Theme) -> impl IntoElement + use<> {
        let from = 8. + RAIL_STEP * self.pill_from.index() as f32;
        let to = 8. + RAIL_STEP * self.nav.index() as f32;
        let pill = div().absolute().left(px(8.)).top(px(to)).size(px(44.)).rounded(px(10.)).bg(th.sel);
        pill.with_animation(("rail-pill", self.pill_seq), motion::once(PILL_MS), move |el, d| {
            el.top(px(from + (to - from) * spring(d)))
        })
    }

    fn rail(&self, th: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let items: Vec<_> = Nav::ALL.iter().map(|n| self.rail_item(*n, th, cx)).collect();
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
            .gap(px(3.))
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
            .child(self.content())
            .child(self.header(&th, cx))
            .child(self.rail(&th, cx))
            .children(self.store.read(cx).toast.as_ref().map(|t| views::toast::toast(t, &th)))
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

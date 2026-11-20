//! Seat page: the CLI login, every seat with its usage, and the "Add seat" form.

mod cards;
mod form;
pub mod limits;

use super::activity::section;
use super::library::{run, toast};
use super::section::{Sec, section_top};
use super::{card, primary_button, tag};
use crate::api;
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use form::SeatForm;
use gpui::{AnyElement, App, Context, Div, Entity, FontWeight, Stateful, Window, div, prelude::*, px};
use rui_core::{SeatInfo, TaskStatus};

/// Seat page: the login status, the seats and the form to add one.
pub struct SeatView {
    store: Entity<Store>,
    form: SeatForm,
    adding: bool,
    /// Seat whose Delete was clicked once and waits for a second click.
    confirm_delete: Option<String>,
}

impl SeatView {
    /// Build the seat view.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let form = SeatForm::new(cx);
        Self { store, form, adding: false, confirm_delete: None }
    }

    fn show_form(&mut self, on: bool, cx: &mut Context<Self>) {
        self.adding = on;
        self.form.reset(cx);
        cx.notify();
    }

    /// Check the form, then add or replace the seat.
    fn save(&mut self, cx: &mut Context<Self>) {
        let body = match self.form.values(cx) {
            Ok(b) => b,
            Err(e) => return self.form_error(e, cx),
        };
        run(cx, move || api::save_seat(body), |v, res, cx| match res {
            Ok(_) => v.changed("Saved the seat", cx),
            Err(e) => v.form_error(e, cx),
        });
    }

    fn form_error(&mut self, e: String, cx: &mut Context<Self>) {
        self.form.error = Some(e);
        cx.notify();
    }

    /// First click arms the delete; the second one removes the seat.
    fn delete_click(&mut self, id: String, cx: &mut Context<Self>) {
        if self.confirm_delete.as_deref() != Some(id.as_str()) {
            self.confirm_delete = Some(id);
            return cx.notify();
        }
        self.confirm_delete = None;
        run(cx, move || api::delete_seat(&id), |v, res, cx| match res {
            Ok(_) => v.changed("Removed the seat", cx),
            Err(e) => toast(&v.store, "Couldn't do that", &e, cx),
        });
    }

    /// After a save or delete: close the form, toast, refresh the seats.
    fn changed(&mut self, title: &str, cx: &mut Context<Self>) {
        self.show_form(false, cx);
        toast(&self.store, title, "", cx);
        self.store.update(cx, |s, cx| s.refresh(cx));
    }

    /// The seat cards, then "Add seat" or the open form.
    fn seats_section(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let seats = self.store.read(cx).seats.clone();
        let mut rows: Vec<AnyElement> = seats.iter().enumerate().map(|(i, s)| self.seat_card(i, s, th, cx)).collect();
        if seats.is_empty() {
            rows.push(div().text_size(px(12.)).text_color(th.muted).child("Loading seats\u{2026}").into_any_element());
        }
        let add = match self.adding {
            true => self.form_card(th, cx).into_any_element(),
            false => primary_button("seat-add", "Add seat", cx).on_click(cx.listener(|this, _, _, cx| this.show_form(true, cx))).into_any_element(),
        };
        rows.push(div().flex().child(add).into_any_element());
        section("Seats", rows)
    }
}

/// One seat for now: taken while any agent runs.
pub fn seats_free(s: &Store) -> usize {
    let running = s.tasks.iter().any(|t| matches!(t.status, TaskStatus::InProgress | TaskStatus::Clarifying));
    usize::from(!running && s.live.tasks.is_empty())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Title of the seat row: the running agent's name, "In use" without one, or "Free seat".
fn seat_title(s: &Store) -> String {
    let Some(running) = s.tasks.iter().find(|t| t.status == TaskStatus::InProgress) else {
        return "Free seat".to_string();
    };
    running.agent.clone().unwrap_or_else(|| "In use".to_string())
}

/// "Claude Code · Max" sub line; the plan is a dash when unknown.
fn seat_sub(seat: &SeatInfo) -> String {
    let plan = seat.subscription_type.as_deref().map(capitalize).unwrap_or_else(|| "—".to_string());
    format!("Claude Code \u{b7} {plan}")
}

/// The small "CC" mark at the left of the seat row.
fn seat_mark(th: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded(px(6.))
        .bg(th.surface_2)
        .text_color(th.fg)
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .child("CC")
}

/// Right-hand tag: signed out, in use, or ready.
fn seat_tag(seat: &SeatInfo, free: bool, th: &Theme) -> Div {
    match (seat.logged_in, free) {
        (false, _) => tag(&th.block, "Signed out"),
        (true, false) => tag(&th.run, "In use"),
        (true, true) => tag(&th.done, "Ready"),
    }
}

/// The one seat row, or the checking note while the account is unknown.
fn seat_row(s: &Store, free: bool, th: &Theme, cx: &App) -> Stateful<Div> {
    let base = div().id("seat-row").flex().flex_col().gap(px(8.)).max_w(px(560.));
    let Some(seat) = s.seat.as_ref() else {
        return base.child(div().text_color(th.muted).child("Checking the CLI account…"));
    };
    let title = div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(seat_title(s));
    let sub = div().text_size(px(11.5)).text_color(th.muted).child(seat_sub(seat));
    let text = div().flex().flex_col().flex_1().child(title).child(sub);
    let row = card(cx).px(px(16.)).py(px(12.)).flex().items_center().gap(px(12.));
    let row = row.child(seat_mark(th)).child(text).child(seat_tag(seat, free, th));
    let err = seat.error.as_ref().map(|e| div().text_size(px(12.)).text_color(th.block.fg).child(e.clone()));
    base.child(row).children(err)
}

/// Lead sentence under the title.
fn lead(free: usize, th: &Theme) -> AnyElement {
    let text = format!("{free} of 1 login is free. Rui uses seats in order and moves to the next one at 90% of its budget.");
    div().id("seat-lead").text_size(px(14.)).text_color(th.fg_2).child(text).into_any_element()
}

impl Render for SeatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let s = self.store.read(cx);
        let free = seats_free(s);
        let row = seat_row(s, free == 1, &th, cx);
        let login = section("Login", vec![row.into_any_element()]);
        let top = section_top(Sec::Cost, Some(lead(free, &th)), None, cx);
        let limits = section("Limits", limits::cards(s.seat.as_ref(), store::now_secs(), &th, cx));
        let seats = self.seats_section(&th, cx);
        let page = div().id("seat-page").flex().flex_col().gap(px(24.)).size_full().overflow_y_scroll().pr(px(40.)).pb(px(28.));
        page.child(top).child(limits).child(login).child(seats)
    }
}

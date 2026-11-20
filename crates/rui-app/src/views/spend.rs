//! Cost → Spend and the Analytics page: both read `GET /costs` for a 7, 30 or 90 day window.

mod analytics;
mod fmt;
mod parts;

use super::library::run;
use super::section::{Sec, section_top};
use super::{chip, primary_button};
use crate::api::{self, Costs};
use crate::store::{self, Store};
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Entity, FontWeight, Render, SharedString, Window, div, prelude::*, px};

const EMPTY: &str = "No spend recorded in this range.";

/// The window the pages sum over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Range {
    Week,
    Month,
    Quarter,
}

impl Range {
    const ALL: [Range; 3] = [Range::Week, Range::Month, Range::Quarter];

    /// Days asked of the daemon.
    pub fn days(self) -> u32 {
        match self {
            Range::Week => 7,
            Range::Month => 30,
            Range::Quarter => 90,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Range::Week => "7 days",
            Range::Month => "30 days",
            Range::Quarter => "90 days",
        }
    }
}

/// Which page the view draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Spend,
    Analytics,
}

/// The spend data for one page, with its range switch.
pub struct SpendView {
    store: Entity<Store>,
    mode: Mode,
    range: Range,
    /// `None` while the first load runs.
    costs: Option<Result<Costs, String>>,
    /// Bumped per load so a slow, older reply never overwrites a newer one.
    seq: u64,
}

impl SpendView {
    pub fn new(store: Entity<Store>, mode: Mode, cx: &mut Context<Self>) -> Self {
        store::observe(&store, cx);
        let mut v = Self { store, mode, range: Range::Month, costs: None, seq: 0 };
        v.load(cx);
        v
    }

    /// Fetch the costs for the current range.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.seq += 1;
        let (seq, days) = (self.seq, self.range.days());
        run(cx, move || api::get_costs(days), move |v, res, cx| {
            if v.seq == seq {
                v.costs = Some(res);
                cx.notify();
            }
        });
    }

    fn set_range(&mut self, r: Range, cx: &mut Context<Self>) {
        if r != self.range {
            self.range = r;
            self.load(cx);
        }
    }

    /// "7 days / 30 days / 90 days", the picked one filled.
    fn range_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = |r: Range, cx: &mut Context<Self>| {
            let id = SharedString::from(format!("spend-range-{}", r.days()));
            let el = if r == self.range { primary_button(id, r.label(), cx) } else { chip(id, r.label(), cx) };
            el.on_click(cx.listener(move |this, _, _, cx| this.set_range(r, cx))).into_any_element()
        };
        let items: Vec<AnyElement> = Range::ALL.into_iter().map(|r| button(r, cx)).collect();
        div().flex().items_center().gap(px(6.)).children(items).into_any_element()
    }

    /// The page body for loaded data.
    fn body(&self, c: &Costs, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if c.total_usd <= 0. && c.total_tokens <= 0 {
            return vec![note(EMPTY, th)];
        }
        match self.mode {
            Mode::Spend => parts::spend_sections(c, self.range.days(), &self.store, th, cx),
            Mode::Analytics => analytics::sections(c, th),
        }
    }

    fn lead(&self, th: &Theme) -> Option<AnyElement> {
        let c = self.costs.as_ref()?.as_ref().ok()?;
        let text = fmt::totals_line(c.total_usd, c.total_tokens);
        Some(div().text_size(px(20.)).font_weight(FontWeight::BOLD).text_color(th.fg).child(text).into_any_element())
    }
}

fn note(text: &str, th: &Theme) -> AnyElement {
    div().text_size(px(13.)).text_color(th.muted).child(text.to_string()).into_any_element()
}

impl Render for SpendView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let sec = if self.mode == Mode::Spend { Sec::Cost } else { Sec::Analytics };
        let top = section_top(sec, self.lead(&th), Some(self.range_switch(cx)), cx);
        let body = match self.costs.clone() {
            None => vec![note("Loading\u{2026}", &th)],
            Some(Err(e)) => vec![note(&format!("Could not load spend: {e}"), &th)],
            Some(Ok(c)) => self.body(&c, &th, cx),
        };
        let page = div().id(SharedString::from(format!("{}-spend-page", sec.key())));
        page.flex().flex_col().gap(px(24.)).size_full().overflow_y_scroll().pr(px(40.)).pb(px(28.)).child(top).children(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_ask_for_their_days() {
        let days: Vec<u32> = Range::ALL.iter().map(|r| r.days()).collect();
        assert_eq!(days, vec![7, 30, 90]);
        assert_eq!(Range::Quarter.label(), "90 days");
    }
}

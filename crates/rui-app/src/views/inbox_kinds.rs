//! Inbox item kinds: the one place that maps a kind string to how it renders, its tag and its buttons.
//! Unknown kinds render generically: title, reason and any detail, with no buttons.

use super::{chip, markdown, primary_button};
use crate::api::InboxItem;
use crate::store::Store;
use crate::theme::{Status, Theme};
use gpui::{AnyElement, App, Div, Entity, FontWeight, Stateful, div, prelude::*, px};

/// Kind strings for email and meeting brief items.
pub const EMAIL_KIND: &str = "email";
pub const BRIEF_KIND: &str = "meeting_brief";

/// How an Inbox item renders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Question,
    Ask,
    Stuck,
    Review,
    Undo,
    Checkpoint,
    Seats,
    Proposal,
    Scout,
    Email,
    Brief,
    Other,
}

/// The renderer for a kind string.
pub fn kind_of(kind: &str) -> Kind {
    match kind {
        "question" => Kind::Question,
        "ask" => Kind::Ask,
        "stuck" => Kind::Stuck,
        "review" => Kind::Review,
        "undo" => Kind::Undo,
        "checkpoint" => Kind::Checkpoint,
        "seats" => Kind::Seats,
        "proposal" => Kind::Proposal,
        "scout" => Kind::Scout,
        EMAIL_KIND => Kind::Email,
        BRIEF_KIND => Kind::Brief,
        _ => Kind::Other,
    }
}

/// Tag text and colour family for a kind.
pub fn tag_of(k: Kind, th: &Theme) -> (&'static str, &Status) {
    match k {
        Kind::Question => ("Question", &th.block),
        Kind::Stuck => ("Stuck", &th.block),
        Kind::Review => ("Review", &th.done),
        Kind::Undo => ("Sending", &th.run),
        Kind::Checkpoint => ("Ready", &th.wait),
        Kind::Ask => ("Asked in chat", &th.think),
        Kind::Seats => ("Seats", &th.block),
        Kind::Proposal => ("Coach", &th.think),
        Kind::Scout => ("Scout", &th.run),
        Kind::Email => ("Email", &th.wait),
        Kind::Brief => ("Meeting", &th.wait),
        Kind::Other => ("Inbox", &th.wait),
    }
}

/// A button an item offers right in its row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemAct {
    Approve,
    Dismiss,
    MakeTask,
    OpenSeats,
    Undo,
    Release,
}

impl ItemAct {
    pub fn label(self) -> &'static str {
        match self {
            ItemAct::Approve => "Approve",
            ItemAct::Dismiss => "Dismiss",
            ItemAct::MakeTask => "Make a task",
            ItemAct::OpenSeats => "Open seats",
            ItemAct::Undo => "Undo",
            ItemAct::Release => "Release",
        }
    }
}

/// The in-row buttons for a kind, main one first. Kinds that open a drawer have none here.
pub fn acts_of(k: Kind) -> Vec<ItemAct> {
    match k {
        Kind::Proposal => vec![ItemAct::Approve, ItemAct::Dismiss],
        Kind::Scout | Kind::Brief => vec![ItemAct::Dismiss],
        Kind::Email => vec![ItemAct::MakeTask, ItemAct::Dismiss],
        Kind::Seats => vec![ItemAct::OpenSeats],
        Kind::Undo => vec![ItemAct::Undo],
        Kind::Checkpoint => vec![ItemAct::Release],
        _ => vec![],
    }
}

/// The item's longer text: markdown for reports, briefs, emails and unknown kinds; a quote for a proposal.
pub fn body(it: &InboxItem, th: &Theme) -> Option<AnyElement> {
    let detail = it.detail.as_deref().filter(|d| !d.trim().is_empty())?;
    match kind_of(&it.kind) {
        Kind::Proposal => Some(quote(detail, th).into_any_element()),
        Kind::Scout | Kind::Brief | Kind::Email | Kind::Other => Some(markdown::render(detail, th.fg, 12.5, th)),
        _ => None,
    }
}

fn quote(text: &str, th: &Theme) -> Div {
    let label = div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.muted).child("ADDS THIS LESSON");
    let text = div().text_size(px(12.5)).text_color(th.fg_2).child(text.to_string());
    div().flex().flex_col().gap(px(4.)).pl(px(10.)).border_l_2().border_color(th.hairline).child(label).child(text)
}

/// The kind's buttons; a click runs the act through the store. The first one is the main button.
pub fn act_buttons(prefix: &str, i: usize, it: &InboxItem, store: &Entity<Store>, cx: &App) -> Vec<Stateful<Div>> {
    let k = kind_of(&it.kind);
    let id = it.id;
    let button = |n: usize, act: ItemAct| {
        let el_id = format!("{prefix}-act-{i}-{n}");
        let el = if n == 0 { primary_button(el_id, act.label(), cx) } else { chip(el_id, act.label(), cx) };
        let store = store.clone();
        el.on_click(move |_, _, cx| store.update(cx, |s, cx| s.inbox_act(k, id, act, cx)))
    };
    acts_of(k).into_iter().enumerate().map(|(n, a)| button(n, a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_renderers_and_unknown_is_generic() {
        assert_eq!(kind_of("proposal"), Kind::Proposal);
        assert_eq!(kind_of("scout"), Kind::Scout);
        assert_eq!(kind_of("seats"), Kind::Seats);
        assert_eq!(kind_of(EMAIL_KIND), Kind::Email);
        assert_eq!(kind_of(BRIEF_KIND), Kind::Brief);
        assert_eq!(kind_of("something_new"), Kind::Other);
        assert!(acts_of(Kind::Other).is_empty());
    }

    #[test]
    fn each_kind_gets_its_buttons() {
        assert_eq!(acts_of(Kind::Proposal), [ItemAct::Approve, ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Scout), [ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Email), [ItemAct::MakeTask, ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Brief), [ItemAct::Dismiss]);
        assert_eq!(acts_of(Kind::Seats), [ItemAct::OpenSeats]);
        assert_eq!(acts_of(Kind::Undo), [ItemAct::Undo]);
        assert_eq!(acts_of(Kind::Checkpoint), [ItemAct::Release]);
        assert!(acts_of(Kind::Question).is_empty());
    }
}

//! Store actions for seats, the Scout and Coach, task limits and Inbox inputs.

use super::{Store, short};
use crate::api;
use crate::views::inbox_kinds::{ItemAct, Kind};
use gpui::Context;

const FAILED: &str = "Couldn't do that";

impl Store {
    /// Run a daemon call; toast `ok` (with `chip`) on success or "Couldn't do that" on failure, then refresh.
    fn act_toast(&mut self, ok: &'static str, chip: String, call: impl FnOnce() -> anyhow::Result<()> + Send + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { call() }).await;
            this.update(cx, |s, cx| {
                match res {
                    Ok(()) => s.show_toast(ok, [chip].into_iter().filter(|c| !c.is_empty()).collect(), cx),
                    Err(e) => s.show_toast(FAILED, vec![short(&e.to_string())], cx),
                }
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Ask the root to switch to the Seat page.
    pub fn go_to_seats(&mut self, cx: &mut Context<Self>) {
        self.open_seats = true;
        cx.notify();
    }

    /// Ask the Scout why a task failed or stalled.
    pub fn diagnose(&mut self, task_id: i64, cx: &mut Context<Self>) {
        self.act_toast("Scout is looking into it", "The report lands in your Inbox".into(), move || api::diagnose(task_id), cx);
    }

    /// Give a task a new attempt limit.
    pub fn set_limit(&mut self, task_id: i64, max_loops: i64, cx: &mut Context<Self>) {
        let chip = format!("Up to {max_loops} tries");
        self.act_toast("Gave it more tries", chip, move || api::set_limit(task_id, max_loops), cx);
    }

    /// Run an Inbox row button for the item `id` of kind `k`.
    pub fn inbox_act(&mut self, k: Kind, id: i64, act: ItemAct, cx: &mut Context<Self>) {
        match (act, k) {
            (ItemAct::Undo, _) => self.undo_answer(id, cx),
            (ItemAct::Release, _) => self.release_step(id, cx),
            (ItemAct::OpenSeats, _) => self.go_to_seats(cx),
            (ItemAct::Approve, _) => self.act_toast("Approved", "Lesson added".into(), move || api::resolve_proposal(id, "approve"), cx),
            (ItemAct::MakeTask, _) => self.act_toast("Made a task", String::new(), move || api::input_action(api::EMAIL_INPUT, id, "make_task"), cx),
            (ItemAct::Dismiss, k) => self.dismiss(k, id, cx),
        }
    }

    fn dismiss(&mut self, k: Kind, id: i64, cx: &mut Context<Self>) {
        let call: Box<dyn FnOnce() -> anyhow::Result<()> + Send> = match k {
            Kind::Proposal => Box::new(move || api::resolve_proposal(id, "dismiss")),
            // A meeting brief is a Scout report, so it is dismissed the same way.
            Kind::Scout | Kind::Brief => Box::new(move || api::dismiss_scout_report(id)),
            Kind::Email => Box::new(move || api::input_action(api::EMAIL_INPUT, id, "dismiss")),
            _ => return,
        };
        self.act_toast("Dismissed", String::new(), call, cx);
    }
}

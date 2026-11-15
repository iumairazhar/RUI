//! Shared app state, refreshed from the daemon once a second.

use crate::api;
use gpui::{Context, Entity};
use rui_core::{ChatMessage, Client, Event, Question, SeatInfo, Task, TaskRun};
use std::collections::HashMap;
use std::time::Duration;

const MAX_EVENTS: usize = 300;
const FIRST_EVENT_LIMIT: usize = 200;

type Core = anyhow::Result<(Vec<Client>, Vec<Task>, Vec<Question>, Vec<ChatMessage>)>;
type Snapshot = (Core, Extras);

/// Holds the latest data from the daemon plus the last error, if any.
pub struct Store {
    pub clients: Vec<Client>,
    pub tasks: Vec<Task>,
    pub questions: Vec<Question>,
    pub chat: Vec<ChatMessage>,
    pub error: Option<String>,
    /// Signed-in seat info; `None` until the first successful fetch.
    pub seat: Option<SeatInfo>,
    /// Latest run per task id.
    pub runs: HashMap<i64, TaskRun>,
    /// Newest events, oldest first (at most 300).
    pub events: Vec<Event>,
    /// Highest event id seen so far.
    pub last_event_id: Option<i64>,
}

/// Extra data fetched each poll; each part may fail on its own.
struct Extras {
    seat: Option<SeatInfo>,
    runs: Option<Vec<TaskRun>>,
    events: Option<Vec<Event>>,
}

fn fetch_extras(after_id: Option<i64>) -> Extras {
    let limit = FIRST_EVENT_LIMIT;
    Extras {
        seat: api::get_seat().ok(),
        runs: api::get_task_runs().ok(),
        events: api::get_events_after(after_id, limit).ok(),
    }
}

fn fetch_core() -> Core {
    Ok((
        api::get_clients()?,
        api::get_tasks()?,
        api::get_questions_open()?,
        api::get_chat()?,
    ))
}

fn fetch_all(after_id: Option<i64>) -> Snapshot {
    (fetch_core(), fetch_extras(after_id))
}

impl Store {
    /// Create the store and start the 1s polling loop.
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                let after = this.read_with(cx, |s, _| s.last_event_id).ok().flatten();
                let snap = cx.background_executor().spawn(async move { fetch_all(after) }).await;
                if this.update(cx, |s, cx| s.apply(snap, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
        })
        .detach();
        Self {
            clients: vec![],
            tasks: vec![],
            questions: vec![],
            chat: vec![],
            error: None,
            seat: None,
            runs: HashMap::new(),
            events: vec![],
            last_event_id: None,
        }
    }

    fn apply_extras(&mut self, extras: Extras) {
        if extras.seat.is_some() {
            self.seat = extras.seat;
        }
        if let Some(runs) = extras.runs {
            self.runs = runs.into_iter().map(|r| (r.task_id, r)).collect();
        }
        let Some(new) = extras.events else { return };
        self.events.extend(new);
        let excess = self.events.len().saturating_sub(MAX_EVENTS);
        self.events.drain(..excess);
        self.last_event_id = self.events.last().map(|e| e.id);
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        let (core, extras) = snap;
        self.apply_extras(extras);
        match core {
            Ok((clients, tasks, questions, chat)) => {
                self.clients = clients;
                self.tasks = tasks;
                self.questions = questions;
                self.chat = chat;
                self.error = None;
            }
            Err(_) => {
                self.error = Some(format!("Daemon not reachable at {}", api::base_url()));
            }
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let after = this.read_with(cx, |s, _| s.last_event_id).ok().flatten();
            let snap = cx.background_executor().spawn(async move { fetch_all(after) }).await;
            this.update(cx, |s, cx| s.apply(snap, cx)).ok();
        })
        .detach();
    }

    /// Post a chat message, then refresh.
    pub fn send_chat(&mut self, text: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = cx.background_executor().spawn(async move { api::post_chat(&text) }).await;
            this.update(cx, |s, cx| s.refresh(cx)).ok();
        })
        .detach();
    }

    /// Answer a question, then refresh.
    pub fn answer(&mut self, question_id: i64, text: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let task = cx
                .background_executor()
                .spawn(async move { api::post_answer(question_id, &text) });
            let _ = task.await;
            this.update(cx, |s, cx| s.refresh(cx)).ok();
        })
        .detach();
    }
}

/// Re-render `view` whenever the store changes.
pub fn observe<V: 'static>(store: &Entity<Store>, cx: &mut Context<V>) {
    cx.observe(store, |_, _, cx| cx.notify()).detach();
}

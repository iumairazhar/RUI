//! Shared app state, refreshed from the daemon once a second.

use crate::api;
use gpui::{Context, Entity};
use rui_core::{ChatMessage, Client, Question, Task};
use std::time::Duration;

type Snapshot = anyhow::Result<(Vec<Client>, Vec<Task>, Vec<Question>, Vec<ChatMessage>)>;

/// Holds the latest data from the daemon plus the last error, if any.
pub struct Store {
    pub clients: Vec<Client>,
    pub tasks: Vec<Task>,
    pub questions: Vec<Question>,
    pub chat: Vec<ChatMessage>,
    pub error: Option<String>,
}

fn fetch_all() -> Snapshot {
    Ok((
        api::get_clients()?,
        api::get_tasks()?,
        api::get_questions_open()?,
        api::get_chat()?,
    ))
}

impl Store {
    /// Create the store and start the 1s polling loop.
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                let snap = cx.background_executor().spawn(async { fetch_all() }).await;
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
        }
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        match snap {
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
            let snap = cx.background_executor().spawn(async { fetch_all() }).await;
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

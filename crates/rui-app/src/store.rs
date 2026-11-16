//! Shared app state, refreshed from the daemon once a second.

use crate::api::{self, AppInfo, Change, LiveSnapshot, SkillInfo};
use gpui::{Context, Entity};
use rui_core::{Chat, ChatMessage, Client, Event, Question, SeatInfo, Task, TaskRun};
use std::collections::HashMap;
use std::time::Duration;

const MAX_EVENTS: usize = 300;
const FIRST_EVENT_LIMIT: usize = 200;

/// Chat list, the chat asked for, the chat resolved as current, and its messages.
type ChatData = (Vec<Chat>, Option<i64>, Option<i64>, Vec<ChatMessage>);
type Core = anyhow::Result<(Vec<Client>, Vec<Task>, Vec<Question>, ChatData)>;
type Snapshot = (Core, Extras);

/// Holds the latest data from the daemon plus the last error, if any.
pub struct Store {
    pub clients: Vec<Client>,
    pub tasks: Vec<Task>,
    pub questions: Vec<Question>,
    /// Messages of `current_chat`, oldest first.
    pub chat: Vec<ChatMessage>,
    /// All chats, newest updated first.
    pub chats: Vec<Chat>,
    /// Chat shown in the chat view; defaults to the newest.
    pub current_chat: Option<i64>,
    pub error: Option<String>,
    /// Last failed owner action (stop, instruct); kept until the next action.
    pub action_error: Option<String>,
    /// Signed-in seat info; `None` until the first successful fetch.
    pub seat: Option<SeatInfo>,
    /// Latest run per task id.
    pub runs: HashMap<i64, TaskRun>,
    /// Newest events, oldest first (at most 300).
    pub events: Vec<Event>,
    /// Highest event id seen so far.
    pub last_event_id: Option<i64>,
    /// Changed files per task, for tasks that were watched.
    pub changes: HashMap<i64, Vec<Change>>,
    /// Full recent event history of the watched task, oldest first.
    pub task_events: HashMap<i64, Vec<Event>>,
    /// File picked in a thread's Files tab: (task id, path, content or error).
    pub file_view: Option<(i64, String, Result<String, String>)>,
    /// What running tasks and chats are doing right now; refreshed every 300ms.
    pub live: LiveSnapshot,
    /// Apps started for projects; refreshed every poll.
    pub apps: Vec<AppInfo>,
    /// Skills from the daemon; loaded at start and on refresh.
    pub skills: Vec<SkillInfo>,
    /// Task whose changes and events are reloaded on every poll.
    watched: Option<i64>,
}

/// Extra data fetched each poll; each part may fail on its own.
struct Extras {
    seat: Option<SeatInfo>,
    runs: Option<Vec<TaskRun>>,
    events: Option<Vec<Event>>,
    apps: Option<Vec<AppInfo>>,
}

fn fetch_extras(after_id: Option<i64>) -> Extras {
    let limit = FIRST_EVENT_LIMIT;
    Extras {
        seat: api::get_seat().ok(),
        runs: api::get_task_runs().ok(),
        events: api::get_events_after(after_id, limit).ok(),
        apps: api::get_apps().ok(),
    }
}

/// The wanted chat if it still exists, else the newest chat.
fn resolve_chat(chats: &[Chat], wanted: Option<i64>) -> Option<i64> {
    let kept = wanted.filter(|id| chats.iter().any(|c| c.id == *id));
    kept.or_else(|| chats.first().map(|c| c.id))
}

fn fetch_chat_data(wanted: Option<i64>) -> anyhow::Result<ChatData> {
    let chats = api::get_chats()?;
    let current = resolve_chat(&chats, wanted);
    let messages = match current {
        Some(id) => api::get_chat(id)?,
        None => vec![],
    };
    Ok((chats, wanted, current, messages))
}

fn fetch_core(wanted: Option<i64>) -> Core {
    Ok((
        api::get_clients()?,
        api::get_tasks()?,
        api::get_questions_open()?,
        fetch_chat_data(wanted)?,
    ))
}

fn fetch_all(after_id: Option<i64>, wanted: Option<i64>) -> Snapshot {
    (fetch_core(wanted), fetch_extras(after_id))
}

/// Create the chat first when there is none, then post; returns the chat id.
fn send_to(current: Option<i64>, text: &str) -> anyhow::Result<i64> {
    let id = match current {
        Some(id) => id,
        None => api::create_chat()?.id,
    };
    api::post_chat(id, text)?;
    Ok(id)
}

impl Store {
    /// Create the store and start the 1s polling loop.
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                let (after, wanted) = this.read_with(cx, |s, _| (s.last_event_id, s.current_chat)).unwrap_or_default();
                let snap = cx.background_executor().spawn(async move { fetch_all(after, wanted) }).await;
                if this.update(cx, |s, cx| s.apply(snap, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
        })
        .detach();
        Self::poll_live(cx);
        Self::load_skills(cx);
        Self {
            clients: vec![],
            tasks: vec![],
            questions: vec![],
            chat: vec![],
            chats: vec![],
            current_chat: None,
            error: None,
            action_error: None,
            seat: None,
            runs: HashMap::new(),
            events: vec![],
            last_event_id: None,
            changes: HashMap::new(),
            task_events: HashMap::new(),
            file_view: None,
            live: LiveSnapshot::default(),
            apps: vec![],
            skills: vec![],
            watched: None,
        }
    }

    /// Every 300ms fetch the live state; keep the old value on error or when unchanged.
    fn poll_live(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let res = cx.background_executor().spawn(async move { api::get_live() }).await;
                let alive = this.update(cx, |s, cx| match res {
                    Ok(live) if live != s.live => {
                        s.live = live;
                        cx.notify();
                    }
                    _ => {}
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(300)).await;
            }
        })
        .detach();
    }

    /// Fetch the skills in the background; keep the old list on error.
    fn load_skills(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { api::get_skills() }).await;
            this.update(cx, |s, cx| {
                if let Ok(list) = res {
                    s.skills = list;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reload the skills (manual refresh).
    pub fn refresh_skills(&mut self, cx: &mut Context<Self>) {
        Self::load_skills(cx);
    }

    fn apply_extras(&mut self, extras: Extras) {
        if let Some(apps) = extras.apps {
            self.apps = apps;
        }
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
            Ok((clients, tasks, questions, chat_data)) => {
                self.clients = clients;
                self.tasks = tasks;
                self.questions = questions;
                self.apply_chat(chat_data);
                self.error = None;
            }
            Err(_) => {
                self.error = Some(format!("Daemon not reachable at {}", api::base_url()));
            }
        }
        self.reload_changes(cx);
        cx.notify();
    }

    /// Store the chat list; drop the messages if the selection changed while fetching.
    fn apply_chat(&mut self, (chats, wanted, current, messages): ChatData) {
        self.chats = chats;
        if self.current_chat.is_some() && self.current_chat != wanted {
            return;
        }
        self.current_chat = current;
        self.chat = messages;
    }

    fn reload_changes(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.watched else { return };
        cx.spawn(async move |this, cx| {
            let fetch = async move { (api::changes(id).ok(), api::task_events(id).ok()) };
            let (changes, events) = cx.background_executor().spawn(fetch).await;
            this.update(cx, |s, cx| {
                let mut changed = false;
                if let Some(list) = changes.filter(|l| s.changes.get(&id) != Some(l)) {
                    s.changes.insert(id, list);
                    changed = true;
                }
                if let Some(list) = events.filter(|l| s.task_events.get(&id) != Some(l)) {
                    s.task_events.insert(id, list);
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Start (or stop, with `None`) reloading changed files for a task on every poll.
    pub fn load_changes(&mut self, id: Option<i64>, cx: &mut Context<Self>) {
        self.watched = id;
        self.reload_changes(cx);
    }

    /// Fetch a file of a task's worktree into `file_view`.
    pub fn load_file(&mut self, id: i64, path: String, cx: &mut Context<Self>) {
        self.file_view = Some((id, path.clone(), Err("Loading\u{2026}".into())));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let res = cx.background_executor().spawn(async move { api::task_file(id, &p) }).await;
            this.update(cx, |s, cx| {
                if !matches!(&s.file_view, Some((i, p, _)) if *i == id && *p == path) {
                    return;
                }
                s.file_view = Some((id, path, file_text(res)));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Run a daemon call in the background, record any failure, then refresh.
    fn act(&mut self, call: impl FnOnce() -> anyhow::Result<()> + Send + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { call() }).await;
            this.update(cx, |s, cx| {
                s.action_error = res.err().map(|e| format!("Request failed: {e}"));
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Stop a task, then refresh.
    pub fn stop_task(&mut self, id: i64, cx: &mut Context<Self>) {
        self.act(move || api::stop(id), cx);
    }

    /// Send new instructions to a task, then refresh.
    pub fn instruct_task(&mut self, id: i64, text: String, cx: &mut Context<Self>) {
        self.act(move || api::instruct(id, &text), cx);
    }

    /// Restart a project's apps, then refresh.
    pub fn restart_apps(&mut self, project: String, cx: &mut Context<Self>) {
        self.act(move || api::restart_apps(&project), cx);
    }

    /// Stop a project's apps, then refresh.
    pub fn stop_apps(&mut self, project: String, cx: &mut Context<Self>) {
        self.act(move || api::stop_apps(&project), cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (after, wanted) = this.read_with(cx, |s, _| (s.last_event_id, s.current_chat)).unwrap_or_default();
            let snap = cx.background_executor().spawn(async move { fetch_all(after, wanted) }).await;
            this.update(cx, |s, cx| s.apply(snap, cx)).ok();
        })
        .detach();
    }

    /// Post a message to the current chat (creating one if none), then refresh.
    pub fn send_chat(&mut self, text: String, cx: &mut Context<Self>) {
        let current = self.current_chat;
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { send_to(current, &text) }).await;
            this.update(cx, |s, cx| {
                if let Ok(id) = res {
                    s.current_chat.get_or_insert(id);
                }
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Show another chat, then refresh.
    pub fn select_chat(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.current_chat == Some(id) {
            return;
        }
        self.current_chat = Some(id);
        self.chat.clear();
        cx.notify();
        self.refresh(cx);
    }

    /// Create a chat in the background, select it, then refresh.
    pub fn new_chat(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { api::create_chat() }).await;
            this.update(cx, |s, cx| {
                if let Ok(chat) = res {
                    s.select_chat(chat.id, cx);
                }
            })
            .ok();
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

fn file_text(res: anyhow::Result<api::TaskFile>) -> Result<String, String> {
    let f = res.map_err(|e| format!("Could not load file: {e}"))?;
    Ok(if f.truncated { format!("{}\n(truncated)", f.content) } else { f.content })
}

/// Re-render `view` whenever the store changes.
pub fn observe<V: 'static>(store: &Entity<Store>, cx: &mut Context<V>) {
    cx.observe(store, |_, _, cx| cx.notify()).detach();
}

//! Shared app state, refreshed from the daemon once a second.

use crate::api::{self, AppInfo, Change, InboxItem, LiveSnapshot, OwnerState, ResultFile, RoleInfo, SkillInfo, ThreadSummary, Undo};
use crate::notify;
use crate::views::decision::CardData;
use crate::views::toast::{SHOW_MS, Toast};
use gpui::{Context, Entity, Global};
use rui_core::{Chat, ChatAsk, ChatMessage, Client, Event, Question, SeatInfo, Task, TaskRun};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

mod support;

const MAX_EVENTS: usize = 300;
const FIRST_EVENT_LIMIT: usize = 200;
/// Answered asks kept for the chat page.
const MAX_ANSWERED_ASKS: usize = 50;

/// Chat list, the chat asked for, the chat resolved as current, and its messages.
type ChatData = (Vec<Chat>, Option<i64>, Option<i64>, Vec<ChatMessage>);
type Core = anyhow::Result<(Vec<Client>, Vec<Task>, Vec<Question>, ChatData)>;
type Snapshot = (Core, Extras);

/// A request to open a produced file in the viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultRequest {
    pub task_id: i64,
    pub path: String,
}

/// An owner answer held in its undo window, with the card it was given on.
#[derive(Debug, Clone, PartialEq)]
pub struct Sending {
    pub undo: Undo,
    pub card: CardData,
}

/// An inbox item's identity: (kind, id).
pub type ItemKey = (String, i64);

/// Unix seconds now.
pub fn now_secs() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The app's store as a global, so click handlers without a view can reach it.
pub struct StoreHandle(pub Entity<Store>);

impl Global for StoreHandle {}

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
    /// Every seat with its usage, from `GET /seats`.
    pub seats: Vec<api::SeatStatus>,
    /// Claude's last reported limits per seat, from `GET /limits`.
    pub limits: Vec<api::SeatLimits>,
    /// Set to ask the root to switch to the Seat page.
    pub open_seats: bool,
    /// Latest run per task id.
    pub runs: HashMap<i64, TaskRun>,
    /// Everything that needs the owner, newest first.
    pub inbox: Vec<InboxItem>,
    /// Status word and reason per task id.
    pub states: HashMap<i64, OwnerState>,
    /// Threads for the Work board, needs-you first; refreshed every poll.
    pub threads: Vec<ThreadSummary>,
    /// Full threads loaded on demand by id (see `load_thread`).
    pub thread_cache: HashMap<i64, api::Thread>,
    /// Newest events, oldest first (at most 300).
    pub events: Vec<Event>,
    /// Highest event id seen so far.
    pub last_event_id: Option<i64>,
    /// Changed files per task, for tasks that were watched.
    pub changes: HashMap<i64, Vec<Change>>,
    /// Files produced per task, for tasks that were watched.
    pub results: HashMap<i64, Vec<ResultFile>>,
    /// Set by a `rui://result` link or a Results row; the root opens the viewer and clears it.
    pub result_request: Option<ResultRequest>,
    /// Full recent event history of the watched task, oldest first.
    pub task_events: HashMap<i64, Vec<Event>>,
    /// File picked in a thread's Files tab: (task id, path, content or error).
    pub file_view: Option<(i64, String, Result<String, String>)>,
    /// What running tasks and chats are doing right now; refreshed every 300ms.
    pub live: LiveSnapshot,
    /// Asks Rui has open, oldest first; refreshed every poll.
    pub asks: Vec<ChatAsk>,
    /// Newest answered asks, oldest first; the chat shows them as done.
    pub answered_asks: Vec<ChatAsk>,
    /// Apps started for projects; refreshed every poll.
    pub apps: Vec<AppInfo>,
    /// Skills from the daemon; loaded at start and on refresh.
    pub skills: Vec<SkillInfo>,
    /// Agent roles from the daemon; loaded with the skills.
    pub roles: Vec<RoleInfo>,
    /// Set by a chat card to ask the root to switch to Threads; the root clears it.
    pub open_threads: bool,
    /// Thread to open once the root has switched to Threads; the root clears it.
    pub open_task: Option<i64>,
    /// Selected sub-tab of each rail section, by section key.
    pub tabs: HashMap<&'static str, usize>,
    /// Toast confirming the last owner action; hides after a few seconds.
    pub toast: Option<Toast>,
    /// Answers still inside their undo window, by question id.
    pub sending: HashMap<i64, Sending>,
    /// Inbox items hidden until a unix time. Kept in memory only: the app has no saved prefs yet.
    pub snoozed: HashMap<ItemKey, i64>,
    /// Set with `open_task` to show the task page's Changes tab; the root clears it.
    pub open_changes: bool,
    /// (updated_at, needs-you count) of each needs-you thread when it was last loaded.
    needs_marks: HashMap<i64, (String, usize)>,
    /// Task whose changes and events are reloaded on every poll.
    watched: Option<i64>,
    /// Thread reloaded into `thread_cache` on every poll (the open task page's).
    watched_thread: Option<i64>,
    /// (kind, id) of inbox items already seen, so only new ones notify.
    seen_inbox: HashSet<(String, i64)>,
    /// The inbox has loaded once; the first load never notifies.
    inbox_loaded: bool,
}

/// Extra data fetched each poll; each part may fail on its own.
struct Extras {
    seat: Option<SeatInfo>,
    seats: Option<Vec<api::SeatStatus>>,
    limits: Option<Vec<api::SeatLimits>>,
    runs: Option<Vec<TaskRun>>,
    inbox: Option<Vec<InboxItem>>,
    states: Option<Vec<OwnerState>>,
    threads: Option<Vec<ThreadSummary>>,
    events: Option<Vec<Event>>,
    apps: Option<Vec<AppInfo>>,
    asks: Option<Vec<ChatAsk>>,
    answered: Option<Vec<ChatAsk>>,
}

fn fetch_extras(after_id: Option<i64>) -> Extras {
    let limit = FIRST_EVENT_LIMIT;
    Extras {
        seat: api::get_seat().ok(),
        seats: api::get_seats().ok(),
        limits: api::get_limits().ok(),
        runs: api::get_task_runs().ok(),
        inbox: api::get_inbox().ok(),
        states: api::get_task_states().ok(),
        threads: api::get_threads().ok(),
        events: api::get_events_after(after_id, limit).ok(),
        apps: api::get_apps().ok(),
        asks: api::get_asks_open().ok(),
        answered: api::get_asks_answered().ok(),
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
            seats: vec![],
            limits: vec![],
            open_seats: false,
            runs: HashMap::new(),
            inbox: vec![],
            states: HashMap::new(),
            threads: vec![],
            thread_cache: HashMap::new(),
            events: vec![],
            last_event_id: None,
            changes: HashMap::new(),
            task_events: HashMap::new(),
            results: HashMap::new(),
            result_request: None,
            file_view: None,
            live: LiveSnapshot::default(),
            asks: vec![],
            answered_asks: vec![],
            apps: vec![],
            skills: vec![],
            roles: vec![],
            open_threads: false,
            open_task: None,
            tabs: HashMap::new(),
            toast: None,
            sending: HashMap::new(),
            snoozed: HashMap::new(),
            open_changes: false,
            needs_marks: HashMap::new(),
            watched: None,
            watched_thread: None,
            seen_inbox: HashSet::new(),
            inbox_loaded: false,
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

    /// Fetch the skills and roles in the background; keep the old lists on error.
    fn load_skills(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let fetch = async move { (api::get_skills(), api::get_roles()) };
            let (skills, roles) = cx.background_executor().spawn(fetch).await;
            this.update(cx, |s, cx| {
                s.skills = skills.unwrap_or_else(|_| std::mem::take(&mut s.skills));
                s.roles = roles.unwrap_or_else(|_| std::mem::take(&mut s.roles));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Ask the root to switch to the Threads page.
    pub fn go_to_threads(&mut self, cx: &mut Context<Self>) {
        self.open_threads = true;
        cx.notify();
    }

    /// Ask the root to switch to Threads and open one task's thread.
    pub fn open_thread(&mut self, id: i64, cx: &mut Context<Self>) {
        self.open_task = Some(id);
        self.go_to_threads(cx);
    }

    /// Ask the root to open a produced file in the viewer.
    pub fn request_result(&mut self, task_id: i64, path: String, cx: &mut Context<Self>) {
        self.result_request = Some(ResultRequest { task_id, path });
        cx.notify();
    }

    /// Reload the skills (manual refresh).
    pub fn refresh_skills(&mut self, cx: &mut Context<Self>) {
        Self::load_skills(cx);
    }

    fn apply_extras(&mut self, extras: Extras) {
        if let Some(apps) = extras.apps {
            self.apps = apps;
        }
        if let Some(asks) = extras.asks {
            self.asks = asks;
        }
        if let Some(done) = extras.answered {
            let skip = done.len().saturating_sub(MAX_ANSWERED_ASKS);
            self.answered_asks = done.into_iter().skip(skip).collect();
        }
        if extras.seat.is_some() {
            self.seat = extras.seat;
        }
        if let Some(seats) = extras.seats {
            self.seats = seats;
        }
        if let Some(limits) = extras.limits {
            self.limits = limits;
        }
        if let Some(runs) = extras.runs {
            self.runs = runs.into_iter().map(|r| (r.task_id, r)).collect();
        }
        if let Some(states) = extras.states {
            self.states = states.into_iter().map(|s| (s.task_id, s)).collect();
        }
        if let Some(threads) = extras.threads {
            self.threads = threads;
        }
        if let Some(inbox) = extras.inbox {
            self.apply_inbox(inbox);
        }
        let Some(new) = extras.events else { return };
        self.events.extend(new);
        let excess = self.events.len().saturating_sub(MAX_EVENTS);
        self.events.drain(..excess);
        self.last_event_id = self.events.last().map(|e| e.id);
    }

    /// Store the inbox; notify for new stuck and review items, except on the first load.
    fn apply_inbox(&mut self, inbox: Vec<InboxItem>) {
        let fresh = notify::fresh(&mut self.seen_inbox, &inbox);
        if std::mem::replace(&mut self.inbox_loaded, true) {
            let items: Vec<(String, String)> = fresh.into_iter().map(|i| (i.title, i.reason)).collect();
            std::thread::spawn(move || items.iter().for_each(|(t, r)| notify::send(t, r)));
        }
        self.inbox = inbox;
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
        self.load_needs_you(cx);
        let now = now_secs();
        self.sending.retain(|_, s| s.undo.run_at > now);
        if let Some(id) = self.watched_thread {
            self.load_thread(id, cx);
        }
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
            let fetch = async move { (api::changes(id).ok(), api::task_events(id).ok(), api::get_results(id).ok()) };
            let (changes, events, results) = cx.background_executor().spawn(fetch).await;
            this.update(cx, |s, cx| {
                let mut changed = false;
                if let Some(list) = changes.filter(|l| s.changes.get(&id) != Some(l)) {
                    s.changes.insert(id, list);
                    changed = true;
                }
                if let Some(list) = results.filter(|l| s.results.get(&id) != Some(l)) {
                    s.results.insert(id, list);
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

    /// Load a thread now and reload it on every poll; `None` stops reloading.
    pub fn watch_thread(&mut self, id: Option<i64>, cx: &mut Context<Self>) {
        self.watched_thread = id;
        if let Some(id) = id {
            self.load_thread(id, cx);
        }
    }

    /// Fetch one full thread into `thread_cache`; keep the old copy on error.
    pub fn load_thread(&mut self, id: i64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { api::get_thread(id) }).await;
            this.update(cx, |s, cx| {
                let Ok(t) = res else { return };
                if s.thread_cache.get(&id) != Some(&t) {
                    s.thread_cache.insert(id, t);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Load every thread that needs the owner and changed since it was last loaded (for Home).
    fn load_needs_you(&mut self, cx: &mut Context<Self>) {
        let mark = |t: &ThreadSummary| (t.updated_at.clone(), t.needs_you_count);
        let due: Vec<(i64, (String, usize))> = self.threads.iter().filter(|t| t.needs_you_count > 0)
            .filter(|t| self.needs_marks.get(&t.id) != Some(&mark(t))).map(|t| (t.id, mark(t))).collect();
        for (id, m) in due {
            self.needs_marks.insert(id, m);
            self.load_thread(id, cx);
        }
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

    /// Show a toast, replacing any current one, and hide it after `SHOW_MS`.
    pub fn show_toast(&mut self, title: &str, chips: Vec<String>, cx: &mut Context<Self>) {
        let seq = self.toast.as_ref().map_or(1, |t| t.seq + 1);
        self.toast = Some(Toast { seq, title: title.into(), chips });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(SHOW_MS)).await;
            this.update(cx, |s, cx| s.hide_toast(seq, cx)).ok();
        })
        .detach();
    }

    fn hide_toast(&mut self, seq: usize, cx: &mut Context<Self>) {
        if self.toast.as_ref().is_some_and(|t| t.seq == seq) {
            self.toast = None;
            cx.notify();
        }
    }

    /// Stop a task and its unfinished subtasks from `source`, toast how many stopped, then refresh.
    pub fn stop_task(&mut self, id: i64, source: &'static str, reason: Option<String>, cx: &mut Context<Self>) {
        self.stop_call(move || api::stop(id, source, reason.as_deref()), cx);
    }

    /// Stop every running team, toast how many stopped, then refresh.
    pub fn stop_all(&mut self, cx: &mut Context<Self>) {
        self.stop_call(|| api::stop_all("Work \u{2192} Stop all"), cx);
    }

    fn stop_call(&mut self, call: impl FnOnce() -> anyhow::Result<Vec<i64>> + Send + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { call() }).await;
            this.update(cx, |s, cx| {
                match res {
                    Ok(ids) => s.show_toast(&stopped_text(ids.len()), vec![], cx),
                    Err(e) => s.action_error = Some(format!("Request failed: {e}")),
                }
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Send new instructions to a task; toast the daemon's note when it sends one, then refresh.
    pub fn instruct_task(&mut self, id: i64, text: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { api::instruct(id, &text) }).await;
            this.update(cx, |s, cx| {
                s.action_error = res.as_ref().err().map(|e| format!("Request failed: {e}"));
                if let Ok(note) = res {
                    s.show_toast("Sent", vec![note.unwrap_or_else(|| format!("Thread {id}"))], cx);
                }
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Review landed work: "Looks good" (`ok`) or send it back with `text`.
    pub fn review_task(&mut self, id: i64, ok: bool, text: Option<String>, cx: &mut Context<Self>) {
        let (title, verdict) = if ok { ("Marked as good", "ok") } else { ("Sent back for changes", "changes") };
        self.show_toast(title, vec![format!("Thread {id}")], cx);
        self.act(move || api::review(id, verdict, text.as_deref()), cx);
    }

    /// Drop a stuck task, then refresh.
    pub fn drop_task(&mut self, id: i64, cx: &mut Context<Self>) {
        self.show_toast("Dropped", vec![format!("Thread {id}")], cx);
        self.act(move || api::drop_task(id), cx);
    }

    /// Release a step held at a checkpoint, then refresh.
    pub fn release_step(&mut self, id: i64, cx: &mut Context<Self>) {
        self.show_toast("Released this step", vec![format!("Task {id}")], cx);
        self.act(move || api::set_checkpoint(id, false), cx);
    }

    /// Ask the root to open a task page on its Changes tab.
    pub fn open_changes(&mut self, id: i64, cx: &mut Context<Self>) {
        self.open_changes = true;
        self.open_thread(id, cx);
    }

    /// Hide an inbox item until `until` (unix seconds).
    pub fn snooze(&mut self, key: ItemKey, until: i64, cx: &mut Context<Self>) {
        self.snoozed.insert(key, until);
        self.show_toast("Snoozed", vec![], cx);
    }

    /// Show a snoozed inbox item again.
    pub fn unsnooze(&mut self, key: &ItemKey, cx: &mut Context<Self>) {
        self.snoozed.remove(key);
        cx.notify();
    }

    /// Retry a stuck task, then refresh.
    pub fn retry_task(&mut self, id: i64, cx: &mut Context<Self>) {
        self.show_toast("Retrying", vec![format!("Thread {id}")], cx);
        self.act(move || api::retry_task(id), cx);
    }

    /// Restart a project's apps, then refresh.
    pub fn restart_apps(&mut self, project: String, cx: &mut Context<Self>) {
        self.show_toast("Restarting apps", vec![project.clone()], cx);
        self.act(move || api::restart_apps(&project), cx);
    }

    /// Stop a project's apps, then refresh.
    pub fn stop_apps(&mut self, project: String, cx: &mut Context<Self>) {
        self.show_toast("Stopped apps", vec![project.clone()], cx);
        self.act(move || api::stop_apps(&project), cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
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
        self.show_toast("New chat started", vec![], cx);
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

    /// Delete a chat in the background; on success drop it and pick the next one, else toast the error.
    pub fn delete_chat(&mut self, id: i64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { api::delete_chat(id) }).await;
            this.update(cx, |s, cx| match res {
                Ok(()) => s.remove_chat(id, cx),
                Err(e) => s.show_toast("Could not delete chat", vec![e.to_string()], cx),
            })
            .ok();
        })
        .detach();
    }

    fn remove_chat(&mut self, id: i64, cx: &mut Context<Self>) {
        self.chats.retain(|c| c.id != id);
        if self.current_chat == Some(id) {
            self.current_chat = self.chats.first().map(|c| c.id);
            self.chat.clear();
        }
        cx.notify();
        self.refresh(cx);
    }

    /// Answer a question (with the owner's reason, if any), then refresh.
    /// On success a toast says who picks it up; a failure (such as 409) lands in `action_error`.
    pub fn answer(&mut self, question_id: i64, text: String, why: Option<String>, cx: &mut Context<Self>) {
        let who = self.questions.iter().find(|q| q.id == question_id).map(pickup_who).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let call = async move { api::post_answer(question_id, &text, why.as_deref()) };
            let res = cx.background_executor().spawn(call).await;
            this.update(cx, |s, cx| {
                s.action_error = res.as_ref().err().map(|e| format!("Request failed: {e}"));
                if res.is_ok() {
                    s.show_toast(&format!("Sent down \u{b7} {who} picks it up now"), vec![], cx);
                }
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }
}

impl Store {
    /// Answer a decision card's question; a held answer is kept in `sending` until its undo window ends.
    pub fn answer_decision(&mut self, card: CardData, text: String, why: Option<String>, cx: &mut Context<Self>) {
        let Some(qid) = card.decision.question_id else { return };
        cx.spawn(async move |this, cx| {
            let call = async move { api::post_answer(qid, &text, why.as_deref()) };
            let res = cx.background_executor().spawn(call).await;
            this.update(cx, |s, cx| s.answered(qid, card, res, cx)).ok();
        })
        .detach();
    }

    fn answered(&mut self, qid: i64, card: CardData, res: anyhow::Result<Option<Undo>>, cx: &mut Context<Self>) {
        self.action_error = res.as_ref().err().map(|e| format!("Request failed: {e}"));
        match res {
            Ok(Some(undo)) => {
                self.sending.insert(qid, Sending { undo, card });
                self.show_toast("Sending your answer\u{2026}", vec!["You can still undo it".into()], cx);
            }
            Ok(None) => self.show_toast("Sent down", vec![], cx),
            Err(e) => self.show_toast("Couldn't send your answer", vec![short(&e.to_string())], cx),
        }
        self.refresh(cx);
    }

    /// Take back a held answer by its event id; the question opens again.
    pub fn undo_answer(&mut self, event_id: i64, cx: &mut Context<Self>) {
        self.sending.retain(|_, s| s.undo.event_id != event_id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { api::cancel_approval(event_id) }).await;
            this.update(cx, |s, cx| {
                match res {
                    Ok(_) => s.show_toast("Answer undone", vec!["The question is open again".into()], cx),
                    Err(e) => s.show_toast("Too late to undo", vec![short(&e.to_string())], cx),
                }
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Answer one of Rui's asks, then refresh. A failure lands in `action_error`.
    pub fn answer_ask(&mut self, ask_id: i64, text: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = cx.background_executor().spawn(async move { api::post_ask_answer(ask_id, &text) }).await;
            this.update(cx, |s, cx| {
                s.action_error = res.as_ref().err().map(|e| format!("Request failed: {e}"));
                if res.is_ok() {
                    s.show_toast("Answered Rui", vec!["It can act now".into()], cx);
                }
                s.refresh(cx);
            })
            .ok();
        })
        .detach();
    }
}

/// Who picks an answer up: the member that first passed the question up.
pub fn pickup_who(q: &Question) -> String {
    match q.ladder.first() {
        Some(rung) => rung.who.clone(),
        None => q.asked_by.as_str().to_string(),
    }
}

/// Error text cut to fit a toast chip.
fn short(e: &str) -> String {
    e.chars().take(80).collect()
}

fn file_text(res: anyhow::Result<api::TaskFile>) -> Result<String, String> {
    let f = res.map_err(|e| format!("Could not load file: {e}"))?;
    Ok(if f.truncated { format!("{}\n(truncated)", f.content) } else { f.content })
}

/// Toast text after a stop: "Stopped · 3 agents stopped".
pub fn stopped_text(n: usize) -> String {
    let noun = if n == 1 { "agent" } else { "agents" };
    format!("Stopped \u{b7} {n} {noun} stopped")
}

/// Re-render `view` whenever the store changes.
pub fn observe<V: 'static>(store: &Entity<Store>, cx: &mut Context<V>) {
    cx.observe(store, |_, _, cx| cx.notify()).detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(ladder: serde_json::Value) -> Question {
        serde_json::from_value(serde_json::json!({"id":1,"task_id":2,"asked_by":"builder","text":"q",
            "answer":null,"status":"open","created_at":"a","ladder":ladder}))
        .unwrap()
    }

    #[test]
    fn stopped_toast_text() {
        assert_eq!(stopped_text(3), "Stopped \u{b7} 3 agents stopped");
        assert_eq!(stopped_text(1), "Stopped \u{b7} 1 agent stopped");
    }

    #[test]
    fn pickup_is_first_rung_or_asker() {
        let q = question(serde_json::json!([{"who":"Backend Engineer 7","why":"db?"},{"who":"Lead","why":"x"}]));
        assert_eq!(pickup_who(&q), "Backend Engineer 7");
        assert_eq!(pickup_who(&question(serde_json::json!([]))), "builder");
    }
}

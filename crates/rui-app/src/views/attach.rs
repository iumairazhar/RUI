//! Files the owner attaches to a message: upload, the Attach button and the file chips.

use super::chip;
use super::library::{run, toast};
use crate::store::Store;
use crate::theme::Theme;
use gpui::{AnyElement, Context, Entity, ExternalPaths, PathPromptOptions, Stateful, Div, div, prelude::*, px};
use rui_core::Event;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Largest file the daemon takes.
const MAX_BYTES: u64 = 20 * 1024 * 1024;
const TEXT_EXTS: [&str; 23] = [
    "txt", "md", "markdown", "csv", "json", "yaml", "yml", "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt",
    "swift", "rb", "php", "html", "css", "sql", "sh",
];
const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// One uploaded file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attached {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub kind: String,
}

/// The files picked for the next message, in the order they were added.
#[derive(Debug, Default)]
pub struct Attachments(Vec<Attached>);

impl Attachments {
    /// Adds a file once (by id).
    pub fn add(&mut self, a: Attached) {
        if !self.0.iter().any(|x| x.id == a.id) {
            self.0.push(a);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.0.retain(|a| a.id != id);
    }

    pub fn items(&self) -> &[Attached] {
        &self.0
    }

    /// The ids to send, clearing the list.
    pub fn take_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut self.0).into_iter().map(|a| a.id).collect()
    }
}

/// "812 B", "4.2 KB" or "1.3 MB".
pub fn size_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        return format!("{bytes} B");
    }
    if b < KB * KB {
        return format!("{:.1} KB", b / KB);
    }
    format!("{:.1} MB", b / KB / KB)
}

/// "text", "pdf" or "image" for a file the daemon takes; `None` otherwise.
pub fn kind_of(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if ext == "pdf" {
        return Some("pdf");
    }
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return Some("image");
    }
    TEXT_EXTS.contains(&ext.as_str()).then_some("text")
}

/// The chip text: "name · size".
pub fn chip_label(a: &Attached) -> String {
    format!("{} \u{b7} {}", a.name, size_label(a.size))
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            let pad = i > c.len();
            out.push(if pad { '=' } else { ABC[(n >> (18 - 6 * i) & 63) as usize] as char });
        }
    }
    out
}

/// Reads the file and posts it to `/uploads`. Errors carry the daemon's message.
pub fn upload(path: &Path) -> anyhow::Result<Attached> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if kind_of(&name).is_none() {
        anyhow::bail!("file type not allowed: {name}");
    }
    if std::fs::metadata(path)?.len() > MAX_BYTES {
        anyhow::bail!("file is larger than 20 MB");
    }
    let data = base64(&std::fs::read(path)?);
    let v = crate::api::library("POST", "/uploads", Some(serde_json::json!({ "name": name, "data_base64": data })))?;
    Ok(serde_json::from_value(v)?)
}

/// The files of each user message in a chat, from its `attachments` events.
pub fn of_messages(events: &[Event], chat_id: i64) -> HashMap<i64, Vec<Attached>> {
    let mut out: HashMap<i64, Vec<Attached>> = HashMap::new();
    for e in events.iter().filter(|e| e.kind == "attachments" && e.chat_id == Some(chat_id)) {
        let Some(mid) = e.payload["message_id"].as_i64() else { continue };
        let files: Vec<Attached> = serde_json::from_value(e.payload["files"].clone()).unwrap_or_default();
        out.entry(mid).or_default().extend(files);
    }
    out
}

/// Where a view keeps its picked files.
pub type Slot<V> = fn(&mut V) -> &mut Attachments;

/// Uploads each path in the background; added files show as chips, failures as a toast.
pub fn add_paths<V: 'static>(cx: &mut Context<V>, store: &Entity<Store>, paths: Vec<PathBuf>, slot: Slot<V>) {
    for p in paths {
        let (store, name) = (store.clone(), p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        run(cx, move || upload(&p), move |v, res, cx| match res {
            Ok(a) => {
                slot(v).add(a);
                cx.notify();
            }
            Err(e) => toast(&store, &format!("Couldn't attach {name}"), &e, cx),
        });
    }
}

/// Opens the file picker (many files) and uploads what the owner picks.
pub fn pick<V: 'static>(cx: &mut Context<V>, store: Entity<Store>, slot: Slot<V>) {
    let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
    cx.spawn(async move |this, cx| {
        let Ok(Ok(Some(paths))) = picked.await else { return };
        this.update(cx, |_, cx| add_paths(cx, &store, paths, slot)).ok();
    })
    .detach();
}

/// The "Attach" button.
pub fn attach_button<V: 'static>(id: &'static str, store: &Entity<Store>, slot: Slot<V>, cx: &mut Context<V>) -> Stateful<Div> {
    let store = store.clone();
    chip(id, "\u{1f4ce} Attach", cx).on_click(cx.listener(move |_, _, _, cx| pick(cx, store.clone(), slot)))
}

/// Makes `el` take dropped files.
pub fn on_drop<V: 'static>(el: Stateful<Div>, store: &Entity<Store>, slot: Slot<V>, cx: &mut Context<V>) -> Stateful<Div> {
    let store = store.clone();
    el.on_drop(cx.listener(move |_, paths: &ExternalPaths, _, cx| add_paths(cx, &store, paths.paths().to_vec(), slot)))
}

fn file_chip(a: &Attached, th: &Theme) -> Div {
    let el = div().flex().items_center().gap(px(6.)).h(px(24.)).px(px(9.)).rounded(px(6.)).bg(th.ctl);
    el.text_size(px(11.5)).text_color(th.fg).child(chip_label(a))
}

/// Removable chips of the picked files; `None` when nothing is picked.
pub fn picked_row<V: 'static>(prefix: &'static str, items: &[Attached], slot: Slot<V>, th: &Theme, cx: &mut Context<V>) -> Option<AnyElement> {
    if items.is_empty() {
        return None;
    }
    let chips = items.iter().enumerate().map(|(i, a)| {
        let id = a.id.clone();
        let x = div().id((prefix, i)).cursor_pointer().text_color(th.muted).child("\u{d7}");
        let x = x.on_click(cx.listener(move |v, _, _, cx| {
            slot(v).remove(&id);
            cx.notify();
        }));
        file_chip(a, th).child(x).into_any_element()
    });
    Some(div().flex().flex_wrap().gap(px(6.)).pb(px(6.)).children(chips).into_any_element())
}

/// Chips of the files a sent message carried.
pub fn sent_row(items: &[Attached], th: &Theme) -> AnyElement {
    let chips = items.iter().map(|a| file_chip(a, th).into_any_element());
    div().flex().flex_wrap().justify_end().gap(px(6.)).pt(px(6.)).children(chips).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, size: u64) -> Attached {
        Attached { id: id.into(), name: format!("{id}.md"), size, kind: "text".into() }
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(size_label(812), "812 B");
        assert_eq!(size_label(4300), "4.2 KB");
        assert_eq!(size_label(1_363_149), "1.3 MB");
        assert_eq!(chip_label(&file("a", 2048)), "a.md \u{b7} 2.0 KB");
    }

    #[test]
    fn kinds_follow_the_extension() {
        assert_eq!(kind_of("Spec.PDF"), Some("pdf"));
        assert_eq!(kind_of("shot.webp"), Some("image"));
        assert_eq!(kind_of("main.rs"), Some("text"));
        assert_eq!(kind_of("setup.exe"), None);
        assert_eq!(kind_of("Makefile"), None);
    }

    #[test]
    fn chip_list_adds_once_removes_and_clears() {
        let mut list = Attachments::default();
        list.add(file("a", 1));
        list.add(file("b", 2));
        list.add(file("a", 1));
        assert_eq!(list.items().len(), 2);
        list.remove("a");
        assert_eq!(list.items()[0].id, "b");
        assert_eq!(list.take_ids(), vec!["b".to_string()]);
        assert!(list.items().is_empty());
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn message_files_come_from_attachments_events() {
        let ev = |chat: i64, mid: i64| Event {
            id: 1,
            task_id: None,
            chat_id: Some(chat),
            agent: "owner".into(),
            kind: "attachments".into(),
            payload: serde_json::json!({"message_id": mid, "files": [file("a", 5)]}),
            created_at: String::new(),
        };
        let m = of_messages(&[ev(1, 7), ev(2, 8)], 1);
        assert_eq!(m.len(), 1);
        assert_eq!(m[&7][0].name, "a.md");
    }
}

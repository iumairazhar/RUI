//! File viewer overlay for the files a team produced.

use super::activity::mono;
use super::library::close_button;
use super::motion::{self, ease, spring};
use super::{chip, markdown};
use crate::api::{self, ResultFile, TaskFile};
use crate::store::{ResultRequest, Store};
use crate::theme::{Theme, theme};
use gpui::{
    AnimationExt, AnyElement, Context, Entity, FocusHandle, FontWeight, KeyDownEvent, Window, div, prelude::*, px,
    relative,
};

/// Lines shown for a plain text file.
const MAX_LINES: usize = 2000;
const TRUNCATED_NOTE: &str = "Showing the first 256 KB \u{2014} open it to see all of it";

/// True for markdown files, which render as a document.
pub fn is_markdown(path: &str) -> bool {
    let ext = path.rsplit('.').next().filter(|_| path.contains('.')).unwrap_or("");
    ["md", "markdown"].iter().any(|m| ext.eq_ignore_ascii_case(m))
}

/// "812 B", "1.5 KB", "2.0 MB".
pub fn size_label(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// Second line of a Results row: the agent's display name and the file size.
pub fn row_meta(agent: &str, size: u64) -> String {
    match agent.is_empty() {
        true => size_label(size),
        false => format!("{agent} \u{b7} {}", size_label(size)),
    }
}

/// The file being shown; `body` is `None` while loading.
struct Open {
    req: ResultRequest,
    abs_path: Option<String>,
    body: Option<Result<TaskFile, String>>,
}

/// Centered viewer panel over a scrim; renders nothing visible until a file is opened.
pub struct ResultsView {
    store: Entity<Store>,
    open: Option<Open>,
    focus: FocusHandle,
    want_focus: bool,
}

impl ResultsView {
    /// Build the viewer.
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        Self { store, open: None, focus: cx.focus_handle(), want_focus: false }
    }

    /// True while a file is shown.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Show `req` and load its content and location in the background.
    pub fn open(&mut self, req: ResultRequest, cx: &mut Context<Self>) {
        self.open = Some(Open { req: req.clone(), abs_path: None, body: None });
        self.want_focus = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = req.clone();
            let fetch = async move { (api::get_result_file(r.task_id, &r.path), api::get_results(r.task_id).ok()) };
            let (file, list) = cx.background_executor().spawn(fetch).await;
            this.update(cx, |v, cx| v.loaded(&req, file, list, cx)).ok();
        })
        .detach();
    }

    fn loaded(&mut self, req: &ResultRequest, file: anyhow::Result<TaskFile>, list: Option<Vec<ResultFile>>, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut().filter(|o| o.req == *req) else { return };
        open.abs_path = list.and_then(|l| l.into_iter().find(|f| f.path == req.path)).map(|f| f.abs_path);
        open.body = Some(file.map_err(|e| format!("Could not load file: {e}")));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        cx.notify();
    }

    fn reveal(&self, flag: Option<&str>) {
        let Some(path) = self.open.as_ref().and_then(|o| o.abs_path.clone()) else { return };
        let mut cmd = std::process::Command::new("open");
        cmd.args(flag).arg(path);
        let _ = cmd.spawn();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if ev.keystroke.key == "escape" {
            self.close(cx);
        }
    }

    fn header(&self, open: &Open, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let title = self.store.read(cx).tasks.iter().find(|t| t.id == open.req.task_id).map(|t| t.title.clone());
        let path = mono(13., th.fg, open.req.path.clone()).font_weight(FontWeight::SEMIBOLD).truncate();
        let sub = title.map(|t| div().text_size(px(11.5)).text_color(th.muted).truncate().child(t));
        let names = div().id("result-names").flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(path).children(sub);
        let can_open = open.abs_path.is_some();
        let open_btn = chip("result-open", "Open", cx).on_click(cx.listener(|this, _, _, _| this.reveal(None)));
        let find_btn = chip("result-reveal", "Show in Finder", cx).on_click(cx.listener(|this, _, _, _| this.reveal(Some("-R"))));
        let close = close_button("result-close", Self::close, th, cx);
        let acts = div().id("result-actions").flex().items_center().gap(px(8.));
        let acts = acts.children(can_open.then_some(open_btn)).children(can_open.then_some(find_btn)).child(close);
        div().id("result-head").flex().items_center().gap(px(12.)).px(px(20.)).py(px(14.)).child(names).child(acts).into_any_element()
    }

    fn body(&self, open: &Open, th: &Theme) -> AnyElement {
        let note = |text: String| div().text_size(px(12.5)).text_color(th.muted).child(text).into_any_element();
        let content = match &open.body {
            None => note("Loading\u{2026}".into()),
            Some(Err(e)) => note(e.clone()),
            Some(Ok(f)) => file_body(&open.req.path, f, th),
        };
        div().id("result-body").flex_1().min_h_0().overflow_y_scroll().p(px(24.)).child(content).into_any_element()
    }
}

fn file_body(path: &str, f: &TaskFile, th: &Theme) -> AnyElement {
    let text = if is_markdown(path) { doc(&f.content, th) } else { plain(&f.content, th) };
    let note = f.truncated.then(|| div().text_size(px(11.5)).text_color(th.muted).child(TRUNCATED_NOTE));
    div().id("result-file").flex().flex_col().gap(px(14.)).child(text).children(note).into_any_element()
}

fn doc(text: &str, th: &Theme) -> AnyElement {
    let page = div().id("result-doc").w_full().max_w(px(760.)).mx_auto();
    page.child(markdown::render(text, th.fg, 14., th)).into_any_element()
}

fn plain(text: &str, th: &Theme) -> AnyElement {
    let line = |l: &str| div().min_h(px(18.)).child(if l.is_empty() { " ".to_string() } else { l.to_string() });
    let lines: Vec<_> = text.lines().take(MAX_LINES).map(line).collect();
    let more = (text.lines().count() > MAX_LINES).then(|| div().child("\u{2026} more lines"));
    let body = div().id("result-text").flex().flex_col().font_family("Menlo").text_size(px(12.)).text_color(th.fg);
    body.line_height(px(18.)).children(lines).children(more).into_any_element()
}

impl Render for ResultsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.want_focus {
            self.want_focus = false;
            window.focus(&self.focus);
        }
        let th = theme(cx).clone();
        let Some(open) = &self.open else { return div().id("result-closed").into_any_element() };
        let panel = div().id("result-panel").occlude().w(relative(0.8)).h(relative(0.8)).flex().flex_col();
        let panel = panel.overflow_hidden().rounded(px(14.)).bg(th.surface).border_1().border_color(th.glass_rim);
        let panel = panel.shadow(th.card_shadow()).child(self.header(open, &th, cx)).child(self.body(open, &th));
        let panel = panel.with_animation("result-pop", motion::once(260), |el, d| {
            el.relative().top(px(10. * (1. - spring(d)))).opacity(ease(d))
        });
        let layer = div().id("result-layer").occlude().absolute().top_0().left_0().right_0().bottom_0();
        let layer = layer.flex().items_center().justify_center().bg(gpui::black().opacity(0.45));
        let layer = layer.track_focus(&self.focus).on_key_down(cx.listener(|this, ev, _, cx| this.key_down(ev, cx)));
        layer.on_click(cx.listener(|this, _, _, cx| this.close(cx))).child(panel).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_check_by_extension() {
        assert!(is_markdown("fide-a1/crash-plan.md"));
        assert!(is_markdown("README.MD") && is_markdown("a.markdown"));
        assert!(!is_markdown("notes.txt") && !is_markdown("md") && !is_markdown("a.md.bak"));
    }

    #[test]
    fn row_label_has_agent_and_size() {
        assert_eq!(row_meta("Backend Engineer 01", 1536), "Backend Engineer 01 \u{b7} 1.5 KB");
        assert_eq!(row_meta("", 812), "812 B");
        assert_eq!(size_label(3 * 1_048_576), "3.0 MB");
    }
}

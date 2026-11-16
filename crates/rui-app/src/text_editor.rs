// Not used by any page yet; drop this once the Skills/Agents pages embed it.
#![allow(dead_code)]

//! Multi-line text editor: a pure `Buffer` plus the `TextEditor` GPUI entity.
//!
//! Usage (not wired into any page yet):
//! ```ignore
//! text_editor::bind_keys(cx); // once, at startup
//! let editor = cx.new(|cx| TextEditor::new("# SKILL.md", cx));
//! cx.subscribe(&editor, |_this, ed, _: &text_editor::Save, cx| {
//!     let text = ed.read(cx).text(); // write it to disk, then:
//!     ed.update(cx, |e, _| e.mark_clean());
//! }).detach();
//! // render: div().h(px(400.)).child(editor.clone())
//! ```

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hsla, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollHandle, SharedString, Style, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, font, point,
    prelude::*, px, relative, size,
};

use crate::theme::theme;

const UNDO_LIMIT: usize = 200;
const FONT: &str = "Menlo";
const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT: f32 = FONT_SIZE * 1.5;

// ───────────────────────── Buffer (pure, no GPUI) ─────────────────────────

#[derive(Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

/// What the last edit was, so typing a word coalesces into one undo step.
#[derive(Clone, Copy, PartialEq)]
enum Last {
    None,
    Word,
    Other,
}

/// Text, cursor, selection and undo history. Offsets are UTF-8 bytes.
pub struct Buffer {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    pref_col: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last: Last,
    revision: u64,
}

fn is_word_char(s: &str) -> bool {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => c.is_alphanumeric() || c == '_',
        _ => false,
    }
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: 0,
            anchor: None,
            pref_col: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last: Last::None,
            revision: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Selected byte range; empty (at the cursor) when nothing is selected.
    pub fn selection(&self) -> Range<usize> {
        let a = self.anchor.unwrap_or(self.cursor);
        a.min(self.cursor)..a.max(self.cursor)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    /// Zero-based (line, column in chars) of the cursor.
    pub fn line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let line = before.matches('\n').count();
        let col = before[self.line_start(self.cursor)..].chars().count();
        (line, col)
    }

    fn line_start(&self, off: usize) -> usize {
        self.text[..off].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, off: usize) -> usize {
        self.text[off..].find('\n').map_or(self.text.len(), |i| off + i)
    }

    fn prev_boundary(&self, off: usize) -> usize {
        self.text[..off].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, off: usize) -> usize {
        let next = self.text[off..].chars().next();
        next.map_or(self.text.len(), |c| off + c.len_utf8())
    }

    // ---- movement ----

    fn move_to(&mut self, off: usize, extend: bool) {
        self.last = Last::None;
        self.pref_col = None;
        let anchor = extend.then(|| self.anchor.unwrap_or(self.cursor));
        self.cursor = off;
        self.anchor = anchor.filter(|a| *a != off);
    }

    pub fn left(&mut self, extend: bool) {
        if !extend && self.anchor.is_some() {
            return self.move_to(self.selection().start, false);
        }
        self.move_to(self.prev_boundary(self.cursor), extend);
    }

    pub fn right(&mut self, extend: bool) {
        if !extend && self.anchor.is_some() {
            return self.move_to(self.selection().end, false);
        }
        self.move_to(self.next_boundary(self.cursor), extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(self.line_start(self.cursor), extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.line_end(self.cursor), extend);
    }

    pub fn up(&mut self, extend: bool) {
        let start = self.line_start(self.cursor);
        if start == 0 {
            return self.move_to(0, extend);
        }
        let want = self.pref_col.unwrap_or(self.line_col().1);
        let prev_start = self.line_start(start - 1);
        self.move_to(self.offset_at_col(prev_start, want), extend);
        self.pref_col = Some(want);
    }

    pub fn down(&mut self, extend: bool) {
        let end = self.line_end(self.cursor);
        if end == self.text.len() {
            return self.move_to(end, extend);
        }
        let want = self.pref_col.unwrap_or(self.line_col().1);
        self.move_to(self.offset_at_col(end + 1, want), extend);
        self.pref_col = Some(want);
    }

    /// Offset of `col` (in chars) on the line starting at `start`, clamped to the line end.
    fn offset_at_col(&self, start: usize, col: usize) -> usize {
        let end = self.line_end(start);
        let rel = self.text[start..end].char_indices().nth(col).map_or(end - start, |(i, _)| i);
        start + rel
    }

    pub fn select_all(&mut self) {
        self.last = Last::None;
        self.pref_col = None;
        self.anchor = (!self.text.is_empty()).then_some(0);
        self.cursor = self.text.len();
    }

    /// Select `range` with the cursor at its end (used by IME).
    pub fn select(&mut self, range: Range<usize>) {
        self.anchor = (range.start != range.end).then_some(range.start);
        self.cursor = range.end;
    }

    // ---- editing ----

    fn snapshot(&self) -> Snapshot {
        Snapshot { text: self.text.clone(), cursor: self.cursor, anchor: self.anchor }
    }

    fn push_undo(&mut self) {
        self.redo.clear();
        self.undo.push(self.snapshot());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Replace `range` with `s`, put the cursor after it. Typing a word coalesces undo steps.
    pub fn replace_range(&mut self, range: Range<usize>, s: &str) {
        if range.is_empty() && s.is_empty() {
            return;
        }
        let word = range.is_empty() && is_word_char(s);
        if !(word && self.last == Last::Word) {
            self.push_undo();
        }
        self.last = if word { Last::Word } else { Last::Other };
        self.text.replace_range(range.clone(), s);
        self.cursor = range.start + s.len();
        self.anchor = None;
        self.pref_col = None;
        self.revision += 1;
    }

    /// Insert at the cursor, replacing the selection.
    pub fn insert(&mut self, s: &str) {
        self.replace_range(self.selection(), s);
    }

    pub fn insert_tab(&mut self) {
        self.insert("  ");
    }

    pub fn backspace(&mut self) {
        let sel = self.selection();
        if !sel.is_empty() {
            return self.replace_range(sel, "");
        }
        self.replace_range(self.prev_boundary(self.cursor)..self.cursor, "");
    }

    pub fn delete(&mut self) {
        let sel = self.selection();
        if !sel.is_empty() {
            return self.replace_range(sel, "");
        }
        self.replace_range(self.cursor..self.next_boundary(self.cursor), "");
    }

    // ---- history ----

    fn restore(&mut self, snap: Snapshot) {
        self.text = snap.text;
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        self.pref_col = None;
        self.last = Last::None;
        self.revision += 1;
    }

    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else { return false };
        self.redo.push(self.snapshot());
        self.restore(snap);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else { return false };
        self.undo.push(self.snapshot());
        self.restore(snap);
        true
    }
}

// ───────────────────────── GPUI entity ─────────────────────────

actions!(
    text_editor,
    [
        EnterKey, Tab, Backspace, Delete, Left, Right, Up, Down, SelectLeft, SelectRight,
        SelectUp, SelectDown, Home, End, SelectHome, SelectEnd, SelectAll, Copy, Cut, Paste,
        Undo, Redo, SaveKey, ShowCharacterPalette,
    ]
);

/// Emitted when the user presses cmd-S.
pub struct Save;

/// Emitted after every user edit that changed the text.
pub struct Changed;

/// Register key bindings for all text editors. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    let c = Some("TextEditor");
    cx.bind_keys([
        KeyBinding::new("enter", EnterKey, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("cmd-left", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("cmd-right", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("cmd-x", Cut, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-z", Undo, c),
        KeyBinding::new("cmd-shift-z", Redo, c),
        KeyBinding::new("cmd-s", SaveKey, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]);
}

/// Wrapped text laid out for one frame, with per-line byte starts and y offsets.
struct Shaped {
    lines: Vec<WrappedLine>,
    starts: Vec<usize>,
    tops: Vec<Pixels>,
    height: Pixels,
}

fn line_height() -> Pixels {
    px(LINE_HEIGHT)
}

fn mark_runs(base: TextRun, marked: Option<Range<usize>>) -> Vec<TextRun> {
    let Some(m) = marked else { return vec![base] };
    let underline = UnderlineStyle { color: Some(base.color), thickness: px(1.), wavy: false };
    let parts = [
        TextRun { len: m.start, ..base.clone() },
        TextRun { len: m.end - m.start, underline: Some(underline), ..base.clone() },
        TextRun { len: base.len - m.end, ..base },
    ];
    parts.into_iter().filter(|r| r.len > 0).collect()
}

impl Shaped {
    fn new(
        text: &str,
        marked: Option<Range<usize>>,
        color: Hsla,
        width: Option<Pixels>,
        window: &Window,
    ) -> Self {
        let base = TextRun {
            len: text.len(),
            font: font(FONT),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = mark_runs(base, marked);
        let shaped = window
            .text_system()
            .shape_text(SharedString::from(text.to_string()), px(FONT_SIZE), &runs, width, None);
        let lines: Vec<WrappedLine> = shaped.map(|l| l.into_vec()).unwrap_or_default();
        let (mut starts, mut tops, mut y, mut at) = (vec![], vec![], px(0.), 0);
        for line in &lines {
            starts.push(at);
            tops.push(y);
            at += line.len() + 1;
            y += line.size(line_height()).height;
        }
        Self { lines, starts, tops, height: y.max(line_height()) }
    }

    fn line_for_offset(&self, off: usize) -> usize {
        self.starts.partition_point(|s| *s <= off).saturating_sub(1)
    }

    /// Position of byte offset `off` relative to the element's top-left.
    fn point_for(&self, off: usize) -> Point<Pixels> {
        let ix = self.line_for_offset(off);
        let Some(line) = self.lines.get(ix) else { return point(px(0.), px(0.)) };
        let local = (off - self.starts[ix]).min(line.len());
        let p = line.position_for_index(local, line_height()).unwrap_or_default();
        point(p.x, p.y + self.tops[ix])
    }

    /// Byte offset closest to `p` (relative to the element's top-left).
    fn offset_for(&self, p: Point<Pixels>) -> usize {
        let ix = self.tops.partition_point(|t| *t <= p.y).saturating_sub(1);
        let Some(line) = self.lines.get(ix) else { return 0 };
        let max_y = line.size(line_height()).height - px(1.);
        let y = (p.y - self.tops[ix]).clamp(px(0.), max_y.max(px(0.)));
        let local = line.closest_index_for_position(point(p.x, y), line_height());
        self.starts[ix] + local.unwrap_or_else(|i| i).min(line.len())
    }

    /// Highlight rectangles (x, y, width) for a non-empty byte range.
    fn selection_rects(&self, sel: &Range<usize>, width: Pixels) -> Vec<(Pixels, Pixels, Pixels)> {
        let lh = line_height();
        let mut rects = vec![];
        for (ix, line) in self.lines.iter().enumerate() {
            let (start, end) = (self.starts[ix], self.starts[ix] + line.len());
            if sel.end < start || sel.start > end {
                continue;
            }
            let (s, e) = (sel.start.max(start), sel.end.min(end));
            let (p1, p2) = (self.point_for(s), self.point_for(e));
            let rows = ((p2.y - p1.y) / lh).round() as usize;
            for r in 0..=rows {
                let x0 = if r == 0 { p1.x } else { px(0.) };
                let mut x1 = if r == rows { p2.x } else { width };
                if r == rows && sel.end > end {
                    x1 += px(6.); // show the selected newline
                }
                rects.push((x0, p1.y + lh * r as f32, x1 - x0));
            }
        }
        rects
    }
}

/// Multi-line editor entity. Emits `Changed` on edits and `Save` on cmd-S.
pub struct TextEditor {
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    buffer: Buffer,
    clean: String,
    marked_range: Option<Range<usize>>,
    shaped: Option<Rc<Shaped>>,
    bounds: Bounds<Pixels>,
    is_selecting: bool,
    reveal: bool,
}

impl EventEmitter<Save> for TextEditor {}
impl EventEmitter<Changed> for TextEditor {}

impl TextEditor {
    pub fn new(text: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            buffer: Buffer::new(text),
            clean: text.to_string(),
            marked_range: None,
            shaped: None,
            bounds: Bounds::default(),
            is_selecting: false,
            reveal: false,
        }
    }

    pub fn text(&self) -> String {
        self.buffer.text().to_string()
    }

    /// Replace the text (cursor to the start, history cleared). Does not mark clean or emit.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer = Buffer::new(text);
        self.marked_range = None;
        cx.notify();
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.text() != self.clean
    }

    /// Treat the current text as saved.
    pub fn mark_clean(&mut self) {
        self.clean = self.buffer.text().to_string();
    }

    /// Cursor position as zero-based (line, column).
    pub fn line_col(&self) -> (usize, usize) {
        self.buffer.line_col()
    }

    // ---- action plumbing ----

    fn nav(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        f(&mut self.buffer);
        self.marked_range = None;
        self.reveal = true;
        cx.notify();
    }

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        let before = self.buffer.revision;
        self.nav(cx, f);
        if self.buffer.revision != before {
            cx.emit(Changed);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selected_text();
        if !sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(sel.to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        self.edit(cx, |b| b.backspace_selection());
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        self.edit(cx, |b| b.insert(&text));
    }

    // ---- mouse ----

    fn offset_at(&self, position: Point<Pixels>) -> usize {
        let Some(shaped) = &self.shaped else { return 0 };
        shaped.offset_for(position - self.bounds.origin)
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let off = self.offset_at(e.position);
        let extend = e.modifiers.shift;
        self.nav(cx, |b| b.move_to(off, extend));
        self.reveal = false;
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }
        let off = self.offset_at(e.position);
        self.nav(cx, |b| b.move_to(off, true));
        self.reveal = false;
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    // ---- UTF-16 helpers for the IME ----

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in self.buffer.text().chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in self.buffer.text().chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }

    fn target_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.buffer.selection())
    }
}

impl Buffer {
    /// Delete the selection if any (used by cut).
    fn backspace_selection(&mut self) {
        let sel = self.selection();
        self.replace_range(sel, "");
    }
}

impl EntityInputHandler for TextEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.buffer.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let reversed = self.buffer.anchor.is_some_and(|a| a > self.buffer.cursor);
        Some(UTF16Selection { range: self.range_to_utf16(&self.buffer.selection()), reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.target_range(range_utf16);
        self.edit(cx, |b| b.replace_range(range, new_text));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.target_range(range_utf16);
        let start = range.start;
        self.edit(cx, |b| b.replace_range(range, new_text));
        self.marked_range = (!new_text.is_empty()).then(|| start..start + new_text.len());
        if let Some(sel) = new_selected_utf16 {
            let sel = self.range_from_utf16(&sel);
            self.buffer.select(start + sel.start..start + sel.end);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let shaped = self.shaped.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let (a, b) = (shaped.point_for(range.start), shaped.point_for(range.end));
        let corner = |p: Point<Pixels>| point(bounds.left() + p.x, bounds.top() + p.y);
        let end = point(b.x.max(a.x + px(1.)), b.y + line_height());
        Some(Bounds::from_corners(corner(a), corner(end)))
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let shaped = self.shaped.as_ref()?;
        let off = shaped.offset_for(p - self.bounds.origin);
        Some(self.offset_to_utf16(off))
    }
}

// ───────────────────────── Element ─────────────────────────

type SharedShaped = Rc<RefCell<Option<Rc<Shaped>>>>;

struct EditorElement {
    editor: Entity<TextEditor>,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = SharedShaped;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let slot: SharedShaped = Rc::new(RefCell::new(None));
        let (out, editor) = (slot.clone(), self.editor.clone());
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let (text, marked) = {
                let e = editor.read(cx);
                (e.buffer.text().to_string(), e.marked_range.clone())
            };
            let shaped = Shaped::new(&text, marked, theme(cx).fg, width, window);
            let result = size(width.unwrap_or_default(), shaped.height);
            *out.borrow_mut() = Some(Rc::new(shaped));
            result
        });
        (id, slot)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        slot: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(shaped) = slot.borrow().clone() else { return };
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        paint_selection(&self.editor, &shaped, bounds, window, cx);
        for (line, top) in shaped.lines.iter().zip(&shaped.tops) {
            let origin = point(bounds.left(), bounds.top() + *top);
            let _ = line.paint(origin, line_height(), TextAlign::Left, None, window, cx);
        }
        if focus.is_focused(window) {
            paint_caret(&self.editor, &shaped, bounds, window, cx);
        }
        self.editor.update(cx, |e, _| {
            e.shaped = Some(shaped);
            e.bounds = bounds;
        });
    }
}

fn paint_selection(
    editor: &Entity<TextEditor>,
    shaped: &Shaped,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let sel = editor.read(cx).buffer.selection();
    if sel.is_empty() {
        return;
    }
    let color = theme(cx).accent_text.opacity(0.3);
    for (x, y, w) in shaped.selection_rects(&sel, bounds.size.width) {
        let origin = point(bounds.left() + x, bounds.top() + y);
        window.paint_quad(fill(Bounds::new(origin, size(w, line_height())), color));
    }
}

fn paint_caret(
    editor: &Entity<TextEditor>,
    shaped: &Shaped,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let (cursor, scroll, reveal) = {
        let e = editor.read(cx);
        (e.buffer.cursor(), e.scroll.clone(), e.reveal)
    };
    let p = shaped.point_for(cursor);
    let rect = Bounds::new(
        point(bounds.left() + p.x, bounds.top() + p.y),
        size(px(2.), line_height()),
    );
    window.paint_quad(fill(rect, theme(cx).accent_text));
    if reveal {
        reveal_caret(&scroll, rect, window);
        editor.update(cx, |e, _| e.reveal = false);
    }
}

/// Nudge the scroll offset so the caret rectangle stays inside the viewport.
fn reveal_caret(scroll: &ScrollHandle, caret: Bounds<Pixels>, window: &mut Window) {
    let view = scroll.bounds();
    let mut offset = scroll.offset();
    if caret.top() < view.top() {
        offset.y += view.top() - caret.top();
    } else if caret.bottom() > view.bottom() {
        offset.y -= caret.bottom() - view.bottom();
    } else {
        return;
    }
    scroll.set_offset(offset);
    window.refresh();
}

impl Render for TextEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        div()
            .key_context("TextEditor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|t, _: &EnterKey, _, cx| t.edit(cx, |b| b.insert("\n"))))
            .on_action(cx.listener(|t, _: &Tab, _, cx| t.edit(cx, |b| b.insert_tab())))
            .on_action(cx.listener(|t, _: &Backspace, _, cx| t.edit(cx, |b| b.backspace())))
            .on_action(cx.listener(|t, _: &Delete, _, cx| t.edit(cx, |b| b.delete())))
            .on_action(cx.listener(|t, _: &Left, _, cx| t.nav(cx, |b| b.left(false))))
            .on_action(cx.listener(|t, _: &Right, _, cx| t.nav(cx, |b| b.right(false))))
            .on_action(cx.listener(|t, _: &Up, _, cx| t.nav(cx, |b| b.up(false))))
            .on_action(cx.listener(|t, _: &Down, _, cx| t.nav(cx, |b| b.down(false))))
            .on_action(cx.listener(|t, _: &SelectLeft, _, cx| t.nav(cx, |b| b.left(true))))
            .on_action(cx.listener(|t, _: &SelectRight, _, cx| t.nav(cx, |b| b.right(true))))
            .on_action(cx.listener(|t, _: &SelectUp, _, cx| t.nav(cx, |b| b.up(true))))
            .on_action(cx.listener(|t, _: &SelectDown, _, cx| t.nav(cx, |b| b.down(true))))
            .on_action(cx.listener(|t, _: &Home, _, cx| t.nav(cx, |b| b.home(false))))
            .on_action(cx.listener(|t, _: &End, _, cx| t.nav(cx, |b| b.end(false))))
            .on_action(cx.listener(|t, _: &SelectHome, _, cx| t.nav(cx, |b| b.home(true))))
            .on_action(cx.listener(|t, _: &SelectEnd, _, cx| t.nav(cx, |b| b.end(true))))
            .on_action(cx.listener(|t, _: &SelectAll, _, cx| t.nav(cx, |b| b.select_all())))
            .on_action(cx.listener(|t, _: &Undo, _, cx| t.edit(cx, |b| {
                b.undo();
            })))
            .on_action(cx.listener(|t, _: &Redo, _, cx| t.edit(cx, |b| {
                b.redo();
            })))
            .on_action(cx.listener(|_, _: &SaveKey, _, cx| cx.emit(Save)))
            .on_action(cx.listener(|_, _: &ShowCharacterPalette, w, _| w.show_character_palette()))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .size_full()
            .bg(th.surface)
            .text_color(th.fg)
            .rounded_md()
            .child(
                div()
                    .id("text-editor-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(px(12.))
                    .child(EditorElement { editor: cx.entity() }),
            )
    }
}

impl Focusable for TextEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ───────────────────────── Tests ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str, cursor: usize) -> Buffer {
        let mut b = Buffer::new(text);
        b.cursor = cursor;
        b
    }

    #[test]
    fn insert_at_cursor() {
        let mut b = buf("held", 3);
        b.insert("l");
        b.insert("o w");
        assert_eq!(b.text(), "hello wd");
        assert_eq!(b.cursor(), 7);
    }

    #[test]
    fn insert_replaces_selection() {
        let mut b = buf("hello world", 0);
        b.right(true);
        b.right(true);
        b.right(true);
        b.right(true);
        b.right(true);
        assert_eq!(b.selected_text(), "hello");
        b.insert("bye");
        assert_eq!(b.text(), "bye world");
        assert!(b.selection().is_empty());
    }

    #[test]
    fn backspace_and_delete() {
        let mut b = buf("abc", 2);
        b.backspace();
        assert_eq!(b.text(), "ac");
        b.delete();
        assert_eq!(b.text(), "a");
        b.delete();
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn multibyte_chars_move_and_delete() {
        let mut b = buf("aé😀b", 0);
        b.right(false);
        b.right(false);
        assert_eq!(b.cursor(), 3);
        b.right(false);
        assert_eq!(b.cursor(), 7);
        b.backspace();
        assert_eq!(b.text(), "aéb");
    }

    #[test]
    fn left_right_collapse_selection() {
        let mut b = buf("abcd", 1);
        b.right(true);
        b.right(true);
        b.left(false);
        assert_eq!(b.cursor(), 1);
        b.right(true);
        b.right(true);
        b.right(false);
        assert_eq!(b.cursor(), 3);
    }

    #[test]
    fn up_down_keep_preferred_column() {
        let mut b = buf("abcdef\nab\nabcdef", 5);
        b.down(false);
        assert_eq!(b.cursor(), 9); // clamped to end of "ab"
        b.down(false);
        assert_eq!(b.line_col(), (2, 5)); // preferred column restored
        b.up(false);
        b.up(false);
        assert_eq!(b.line_col(), (0, 5));
    }

    #[test]
    fn up_on_first_line_goes_home_down_on_last_goes_end() {
        let mut b = buf("ab\ncd", 1);
        b.up(false);
        assert_eq!(b.cursor(), 0);
        b.down(false);
        b.down(false);
        assert_eq!(b.cursor(), 5);
    }

    #[test]
    fn home_end_use_current_line() {
        let mut b = buf("one\ntwo\nthree", 5);
        b.home(false);
        assert_eq!(b.cursor(), 4);
        b.end(false);
        assert_eq!(b.cursor(), 7);
    }

    #[test]
    fn shift_extends_selection_across_lines() {
        let mut b = buf("ab\ncd", 1);
        b.down(true);
        assert_eq!(b.selected_text(), "b\nc");
        b.up(true);
        assert!(b.selection().is_empty());
    }

    #[test]
    fn select_all_then_delete() {
        let mut b = buf("a\nb\nc", 2);
        b.select_all();
        assert_eq!(b.selected_text(), "a\nb\nc");
        b.delete();
        assert_eq!(b.text(), "");
        assert!(b.undo());
        assert_eq!(b.text(), "a\nb\nc");
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut b = buf("", 0);
        b.insert("ab");
        b.insert("\n");
        b.insert("c");
        assert!(b.undo());
        assert_eq!(b.text(), "ab\n");
        assert!(b.undo());
        assert!(b.undo());
        assert_eq!(b.text(), "");
        assert!(!b.undo());
        assert!(b.redo());
        assert_eq!(b.text(), "ab");
        assert!(b.redo() && b.redo());
        assert_eq!(b.text(), "ab\nc");
        assert!(!b.redo());
    }

    #[test]
    fn typing_a_word_is_one_undo_step() {
        let mut b = buf("", 0);
        for c in ["h", "e", "l", "l", "o", " ", "w", "o"] {
            b.insert(c);
        }
        b.undo();
        assert_eq!(b.text(), "hello ");
        b.undo();
        assert_eq!(b.text(), "hello");
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn moving_breaks_coalescing_and_edit_clears_redo() {
        let mut b = buf("", 0);
        b.insert("a");
        b.left(false);
        b.right(false);
        b.insert("b");
        b.undo();
        assert_eq!(b.text(), "a");
        b.insert("x");
        assert!(!b.redo());
    }

    #[test]
    fn undo_stack_is_capped() {
        let mut b = buf("", 0);
        for _ in 0..300 {
            b.insert("\n");
        }
        assert_eq!(b.undo.len(), UNDO_LIMIT);
    }

    #[test]
    fn tab_inserts_two_spaces() {
        let mut b = buf("x", 0);
        b.insert_tab();
        assert_eq!(b.text(), "  x");
        assert_eq!(b.cursor(), 2);
    }

    #[test]
    fn line_col_counts_chars() {
        let b = buf("é\nabc", 6);
        assert_eq!(b.line_col(), (1, 3));
    }
}

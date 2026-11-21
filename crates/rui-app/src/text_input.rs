//! Adapted from Zed's gpui examples/input.rs (Apache-2.0).

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
    WrappedLine, actions,
    div, fill, point, prelude::*, px, relative, size,
};

use crate::theme::theme;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Enter,
        Newline,
        Up,
        Down,
        SelectUp,
        SelectDown,
    ]
);

/// Emitted when the user presses Enter in a text input.
pub struct Submit;

/// A multi-line input grows to this many rows, then scrolls.
pub const MAX_ROWS: usize = 8;

/// Register the key bindings for all text inputs. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Enter, Some("TextInput")),
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("cmd-c", Copy, Some("TextInput")),
        KeyBinding::new("cmd-x", Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextInput")),
        KeyBinding::new("shift-enter", Newline, Some("TextInput && multiline")),
        KeyBinding::new("up", Up, Some("TextInput && multiline")),
        KeyBinding::new("down", Down, Some("TextInput && multiline")),
        KeyBinding::new("shift-up", SelectUp, Some("TextInput && multiline")),
        KeyBinding::new("shift-down", SelectDown, Some("TextInput && multiline")),
    ]);
}

/// Text input: one line by default, or a wrapping composer that grows (see [`TextInput::multiline`]).
pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    /// Where the text was painted: the field's box shifted left by `scroll_x`.
    last_bounds: Option<Bounds<Pixels>>,
    /// How far the line is scrolled left so the cursor stays inside the field.
    scroll_x: f32,
    is_selecting: bool,
    /// Soft-wrap and grow up to [`MAX_ROWS`]; Shift+Enter adds a newline.
    multiline: bool,
    /// The wrapped layout from the last paint (multi-line only).
    last_wrapped: Option<Wrapped>,
    /// How far the wrapped text is scrolled up so the cursor stays in view.
    scroll_y: f32,
}

impl TextInput {
    /// Create an empty input with a placeholder.
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.to_string().into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_x: 0.,
            is_selecting: false,
            multiline: false,
            last_wrapped: None,
            scroll_y: 0.,
        }
    }

    /// Make this a composer: text wraps, the box grows to [`MAX_ROWS`], Shift+Enter adds a newline.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Current text.
    pub fn content(&self) -> String {
        self.content.to_string()
    }

    /// Clear the text and notify.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.reset();
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        self.on_enter(false, window, cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.on_enter(true, window, cx);
    }

    fn on_enter(&mut self, shift: bool, window: &mut Window, cx: &mut Context<Self>) {
        if enter_inserts_newline(shift, self.multiline) {
            return self.replace_text_in_range(None, "\n", window, cx);
        }
        cx.emit(Submit);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(-1.);
        self.move_to(target, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(1.);
        self.move_to(target, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(-1.);
        self.select_to(target, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(1.);
        self.select_to(target, cx);
    }

    /// The offset one visual row above (`dir` -1) or below (+1) the cursor, keeping its x.
    fn vertical_target(&self, dir: f32) -> usize {
        let len = self.content.len();
        let Some(w) = self.last_wrapped.as_ref().filter(|_| len > 0) else {
            return if dir < 0. { 0 } else { len };
        };
        let p = w.position(self.cursor_offset());
        let y = p.y + w.line_height * dir;
        if y < px(0.) {
            return 0;
        }
        if y >= w.line_height * w.total_rows() as f32 {
            return len;
        }
        w.index_at(point(p.x, y)).min(len)
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace("\r\n", "\n");
            let text = if self.multiline { text } else { text.replace('\n', " ") };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        if let (Some(bounds), Some(w)) = (self.last_bounds.as_ref(), self.last_wrapped.as_ref()) {
            let local = position - bounds.origin;
            return w.index_at(local).min(self.content.len());
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn reset(&mut self) {
        self.content = "".into();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;
        self.is_selecting = false;
        self.last_wrapped = None;
        self.scroll_y = 0.;
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        if let (Some(w), Some(b)) = (self.last_wrapped.as_ref(), self.last_bounds) {
            return Some(w.range_bounds(range, b.origin));
        }
        let last_layout = self.last_layout.as_ref()?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if let (Some(w), Some(b)) = (self.last_wrapped.as_ref(), self.last_bounds) {
            let ix = w.index_at(point - b.origin).min(self.content.len());
            return Some(self.offset_to_utf16(ix));
        }
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        assert_eq!(last_layout.text, self.content);
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

/// The next sideways scroll: as little as possible, keeping the cursor in a field `width` wide
/// and not scrolling past the end of a line `text_w` wide.
fn next_scroll(prev: f32, cursor_x: f32, text_w: f32, width: f32) -> f32 {
    let margin = 2.;
    let mut s = prev.min((text_w + margin - width).max(0.));
    if cursor_x < s {
        s = cursor_x;
    }
    if cursor_x + margin > s + width {
        s = cursor_x + margin - width;
    }
    s.max(0.)
}

/// The next vertical scroll: as little as possible, keeping the cursor row (top `caret_y`,
/// `line_h` tall) inside a view `view_h` tall over text `text_h` tall.
fn next_scroll_y(prev: f32, caret_y: f32, line_h: f32, text_h: f32, view_h: f32) -> f32 {
    let mut s = prev.min((text_h - view_h).max(0.));
    if caret_y < s {
        s = caret_y;
    }
    if caret_y + line_h > s + view_h {
        s = caret_y + line_h - view_h;
    }
    s.max(0.)
}

/// How many rows a multi-line input shows: at least one, at most [`MAX_ROWS`].
fn visible_rows(rows: usize) -> usize {
    rows.clamp(1, MAX_ROWS)
}

/// Whether an Enter press (with Shift when `shift`) adds a newline rather than submitting.
fn enter_inserts_newline(shift: bool, multiline: bool) -> bool {
    shift && multiline
}

/// The byte offset where each hard line (split on `\n`) of `text` starts.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect()
}

/// Which hard line holds visual `row`, and that line's first row, given each line's row count.
fn line_for_row(rows: &[usize], row: usize) -> (usize, usize) {
    let mut first = 0;
    for (k, n) in rows.iter().enumerate() {
        if row < first + n {
            return (k, first);
        }
        first += n;
    }
    let last = rows.last().copied().unwrap_or(0);
    (rows.len().saturating_sub(1), first - last)
}

/// Soft-wrapped text: one [`WrappedLine`] per hard line, with where each starts.
struct Wrapped {
    lines: Vec<WrappedLine>,
    starts: Vec<usize>,
    rows: Vec<usize>,
    line_height: Pixels,
}

impl Wrapped {
    fn new(lines: Vec<WrappedLine>, text: &str, line_height: Pixels) -> Self {
        let rows = lines.iter().map(|l| l.wrap_boundaries().len() + 1).collect();
        Self { lines, starts: line_starts(text), rows, line_height }
    }

    fn total_rows(&self) -> usize {
        self.rows.iter().sum()
    }

    /// Top-left of the cursor slot at byte `ix`, relative to the text's origin.
    fn position(&self, ix: usize) -> Point<Pixels> {
        let k = self.starts.partition_point(|s| *s <= ix).saturating_sub(1);
        let Some(line) = self.lines.get(k) else {
            return point(px(0.), px(0.));
        };
        let first: usize = self.rows[..k].iter().sum();
        let p = line.position_for_index(ix - self.starts[k], self.line_height).unwrap_or_default();
        point(p.x, p.y + self.line_height * first as f32)
    }

    /// The byte offset closest to `p`, relative to the text's origin.
    fn index_at(&self, p: Point<Pixels>) -> usize {
        if p.y < px(0.) {
            return 0;
        }
        let row = (p.y / self.line_height) as usize;
        let (k, first) = line_for_row(&self.rows, row);
        let Some(line) = self.lines.get(k) else {
            return 0;
        };
        let y = self.line_height * ((row - first) as f32 + 0.5);
        let local = point(p.x.max(px(0.)), y.min(self.line_height * (self.rows[k] as f32 - 0.5)));
        let ix = line.closest_index_for_position(local, self.line_height).unwrap_or_else(|e| e);
        self.starts[k] + ix
    }

    /// Screen bounds for `range` on its first row, for the IME popup.
    fn range_bounds(&self, range: Range<usize>, origin: Point<Pixels>) -> Bounds<Pixels> {
        let (a, b) = (self.position(range.start), self.position(range.end));
        let right = if a.y == b.y { b.x } else { a.x + px(2.) };
        Bounds::from_corners(origin + a, origin + point(right, a.y + self.line_height))
    }

    /// One highlight quad per visual row the selection covers.
    fn selection(&self, range: &Range<usize>, origin: Point<Pixels>, width: Pixels, color: gpui::Hsla) -> Vec<PaintQuad> {
        let (a, b) = (self.position(range.start), self.position(range.end));
        let lh = self.line_height;
        let quad = |x0: Pixels, x1: Pixels, y: Pixels| fill(Bounds::from_corners(origin + point(x0, y), origin + point(x1, y + lh)), color);
        if a.y == b.y {
            return vec![quad(a.x, b.x, a.y)];
        }
        let mut quads = vec![quad(a.x, width, a.y)];
        let mut y = a.y + lh;
        while y < b.y {
            quads.push(quad(px(0.), width, y));
            y += lh;
        }
        quads.push(quad(px(0.), b.x, b.y));
        quads
    }
}

struct PrepaintState {
    /// The field's box, to clip the text to.
    clip: Bounds<Pixels>,
    /// The field's box shifted by the scroll; text, cursor and selection are placed in it.
    shifted: Bounds<Pixels>,
    scroll_x: f32,
    scroll_y: f32,
    line: Option<ShapedLine>,
    wrapped: Option<Wrapped>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

/// The text to show (content, or the placeholder when empty) and its color runs, with
/// IME marked text underlined.
fn display_runs(input: &TextInput, window: &Window, cx: &App) -> (SharedString, Vec<TextRun>) {
    let style = window.text_style();
    let (text, color) = if input.content.is_empty() {
        (input.placeholder.clone(), theme(cx).muted)
    } else {
        (input.content.clone(), style.color)
    };
    let run = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
    let Some(marked) = input.marked_range.as_ref() else {
        return (text, vec![run]);
    };
    let underline = Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false });
    let runs = vec![
        TextRun { len: marked.start, ..run.clone() },
        TextRun { len: marked.end - marked.start, underline, ..run.clone() },
        TextRun { len: text.len() - marked.end, ..run },
    ];
    (text, runs.into_iter().filter(|r| r.len > 0).collect())
}

fn shape_wrapped(window: &Window, text: &SharedString, runs: &[TextRun], width: Option<Pixels>) -> Vec<WrappedLine> {
    let font_size = window.text_style().font_size.to_pixels(window.rem_size());
    let lines = window.text_system().shape_text(text.clone(), font_size, runs, width, None);
    lines.map(|l| l.into_vec()).unwrap_or_default()
}

impl TextElement {
    /// Ask for a box as wide as the parent and as tall as the wrapped text (1 to `MAX_ROWS` rows).
    fn multiline_layout(&self, mut style: Style, window: &mut Window, cx: &mut App) -> LayoutId {
        let (text, runs) = display_runs(self.input.read(cx), window, cx);
        let line_height = window.line_height();
        style.size.height = gpui::Length::Auto;
        window.request_measured_layout(style, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let rows = shape_wrapped(window, &text, &runs, width).iter().map(|l| l.wrap_boundaries().len() + 1).sum();
            size(width.unwrap_or(px(0.)), line_height * visible_rows(rows) as f32)
        })
    }

    fn prepaint_multiline(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) -> PrepaintState {
        let th = theme(cx).clone();
        let input = self.input.read(cx);
        let (text, runs) = display_runs(input, window, cx);
        let lh = window.line_height();
        let wrapped = Wrapped::new(shape_wrapped(window, &text, &runs, Some(bounds.size.width)), &text, lh);
        let caret = wrapped.position(input.cursor_offset());
        let text_h = f32::from(lh) * wrapped.total_rows() as f32;
        let scroll_y = next_scroll_y(input.scroll_y, f32::from(caret.y), f32::from(lh), text_h, f32::from(bounds.size.height));
        let shifted = Bounds::new(point(bounds.left(), bounds.top() - px(scroll_y)), bounds.size);
        let range = input.selected_range.clone();
        let (selection, cursor) = if range.is_empty() {
            let bar = Bounds::new(shifted.origin + caret, size(px(2.), lh));
            (Vec::new(), Some(fill(bar, th.accent_text)))
        } else {
            (wrapped.selection(&range, shifted.origin, bounds.size.width, th.accent_text.opacity(0.3)), None)
        };
        PrepaintState { clip: bounds, shifted, scroll_x: 0., scroll_y, line: None, wrapped: Some(wrapped), cursor, selection }
    }

    fn paint_wrapped(wrapped: &Wrapped, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let mut y = origin.y;
        for (line, rows) in wrapped.lines.iter().zip(&wrapped.rows) {
            line.paint(point(origin.x, y), wrapped.line_height, TextAlign::Left, None, window, cx).ok();
            y += wrapped.line_height * *rows as f32;
        }
    }
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if self.input.read(cx).multiline {
            return (self.multiline_layout(style, window, cx), ());
        }
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if self.input.read(cx).multiline {
            return self.prepaint_multiline(bounds, window, cx);
        }
        let th = theme(cx).clone();
        let input = self.input.read(cx);
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let (display_text, runs) = display_runs(input, window, cx);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let scroll_x = next_scroll(input.scroll_x, f32::from(cursor_pos), f32::from(line.width), f32::from(bounds.size.width));
        let clip = bounds;
        let bounds = Bounds::new(point(bounds.left() - px(scroll_x), bounds.top()), size(bounds.size.width + px(scroll_x), bounds.size.height));
        let (selection, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    th.accent_text,
                )),
            )
        } else {
            (
                vec![fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    th.accent_text.opacity(0.3),
                )],
                None,
            )
        };
        PrepaintState {
            clip,
            shifted: bounds,
            scroll_x,
            scroll_y: 0.,
            line: Some(line),
            wrapped: None,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let (shifted, scroll_x, scroll_y) = (prepaint.shifted, prepaint.scroll_x, prepaint.scroll_y);
        let line = prepaint.line.take();
        let wrapped = prepaint.wrapped.take();
        let focused = focus_handle.is_focused(window);
        // Clip to the field so text never paints past its edge.
        window.with_content_mask(Some(ContentMask { bounds: prepaint.clip }), |window| {
            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad)
            }
            if let Some(line) = line.as_ref() {
                line.paint(shifted.origin, window.line_height(), window, cx).ok();
            }
            if let Some(wrapped) = wrapped.as_ref() {
                Self::paint_wrapped(wrapped, shifted.origin, window, cx);
            }
            if focused && let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });

        self.input.update(cx, |input, _cx| {
            input.last_layout = line;
            input.last_wrapped = wrapped;
            input.last_bounds = Some(shifted);
            input.scroll_x = scroll_x;
            input.scroll_y = scroll_y;
        });
    }
}

impl EventEmitter<Submit> for TextInput {}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context(if self.multiline { "TextInput multiline" } else { "TextInput" })
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .bg(theme(cx).surface)
            .text_color(theme(cx).fg)
            .line_height(px(20.))
            .text_size(px(14.))
            .rounded_md()
            .child(
                div()
                    .when(!self.multiline, |d| d.h(px(20. + 8. * 2.)))
                    .w_full()
                    .p(px(8.))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::{enter_inserts_newline, line_for_row, line_starts, next_scroll, next_scroll_y, visible_rows};

    #[test]
    fn rows_clamp_between_one_and_eight() {
        assert_eq!(visible_rows(0), 1);
        assert_eq!(visible_rows(3), 3);
        assert_eq!(visible_rows(8), 8);
        assert_eq!(visible_rows(40), 8);
    }

    #[test]
    fn shift_enter_adds_a_newline_only_in_a_composer() {
        assert!(enter_inserts_newline(true, true));
        assert!(!enter_inserts_newline(false, true));
        assert!(!enter_inserts_newline(true, false));
        assert!(!enter_inserts_newline(false, false));
    }

    #[test]
    fn hard_lines_and_rows_map_both_ways() {
        assert_eq!(line_starts("ab\ncd\n"), vec![0, 3, 6]);
        assert_eq!(line_starts(""), vec![0]);
        let rows = [2, 1, 3];
        assert_eq!(line_for_row(&rows, 0), (0, 0));
        assert_eq!(line_for_row(&rows, 1), (0, 0));
        assert_eq!(line_for_row(&rows, 2), (1, 2));
        assert_eq!(line_for_row(&rows, 5), (2, 3));
        assert_eq!(line_for_row(&rows, 9), (2, 3));
    }

    #[test]
    fn vertical_scroll_keeps_the_cursor_row_in_view() {
        assert_eq!(next_scroll_y(0., 0., 20., 100., 160.), 0.);
        assert_eq!(next_scroll_y(0., 180., 20., 200., 160.), 40.);
        assert_eq!(next_scroll_y(40., 20., 20., 200., 160.), 20.);
        assert_eq!(next_scroll_y(40., 0., 20., 20., 160.), 0.);
    }

    #[test]
    fn scroll_keeps_the_cursor_in_view() {
        assert_eq!(next_scroll(0., 50., 50., 100.), 0.);
        assert_eq!(next_scroll(0., 150., 150., 100.), 52.);
        assert_eq!(next_scroll(52., 10., 150., 100.), 10.);
        assert_eq!(next_scroll(52., 100., 120., 100.), 22.);
        assert_eq!(next_scroll(52., 0., 0., 100.), 0.);
    }
}

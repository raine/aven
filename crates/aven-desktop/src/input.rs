//! Text editing built directly on GPUI.
//!
//! Derived from Zed's `crates/gpui/examples/input.rs`, Copyright 2022 - 2025
//! Zed Industries, Inc., licensed under Apache-2.0. Extended with soft
//! wrapping, hard lines, vertical movement, word and line movement, undo,
//! click counts, drag selection outside the element, horizontal and vertical
//! scrolling, a blinking caret and a read-only selectable mode.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    FontWeight, GlobalElementId, Hsla, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollEvent, SharedString, Size, Style,
    TextRun, TextStyle, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill,
    point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::markdown::{Quote, Rendered, Span};
use crate::theme::theme;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToParagraphEnd,
        Left,
        Right,
        Up,
        Down,
        WordLeft,
        WordRight,
        LineStart,
        LineEnd,
        ParagraphStart,
        ParagraphEnd,
        DocStart,
        DocEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectLineStart,
        SelectLineEnd,
        SelectParagraphStart,
        SelectParagraphEnd,
        SelectDocStart,
        SelectDocEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Submit,
        InsertNewline,
        Cancel,
        ShowCharacterPalette,
    ]
);

pub fn key_bindings() -> Vec<KeyBinding> {
    let c = Some("TextInput");
    vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("ctrl-h", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-d", Delete, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("alt-delete", DeleteWordRight, c),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, c),
        KeyBinding::new("ctrl-k", DeleteToParagraphEnd, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("ctrl-b", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("ctrl-f", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("ctrl-p", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("ctrl-n", Down, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("cmd-left", LineStart, c),
        KeyBinding::new("cmd-right", LineEnd, c),
        KeyBinding::new("home", LineStart, c),
        KeyBinding::new("end", LineEnd, c),
        KeyBinding::new("ctrl-a", ParagraphStart, c),
        KeyBinding::new("ctrl-e", ParagraphEnd, c),
        KeyBinding::new("alt-up", ParagraphStart, c),
        KeyBinding::new("alt-down", ParagraphEnd, c),
        KeyBinding::new("cmd-up", DocStart, c),
        KeyBinding::new("cmd-down", DocEnd, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("cmd-shift-left", SelectLineStart, c),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("alt-shift-up", SelectParagraphStart, c),
        KeyBinding::new("alt-shift-down", SelectParagraphEnd, c),
        KeyBinding::new("cmd-shift-up", SelectDocStart, c),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("cmd-x", Cut, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-z", Undo, c),
        KeyBinding::new("cmd-shift-z", Redo, c),
        KeyBinding::new("enter", Submit, c),
        KeyBinding::new("shift-enter", InsertNewline, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One line, scrolling sideways.
    Single,
    /// Soft wraps, never holds a newline.
    Wrap,
    /// Soft wraps and holds hard lines.
    Multi,
}

pub enum EditorEvent {
    Changed,
    Submit,
    Cancel,
    Blur,
    Up,
    Down,
    /// A paste the editor cannot hold, for a surface that takes images.
    PastedImage(PastedImage),
}

/// An image from the clipboard: its bytes, or the file it was copied from.
#[derive(Clone)]
pub enum PastedImage {
    Bytes(gpui::Image),
    Path(std::path::PathBuf),
}

/// An image the clipboard holds, if any.
pub fn clipboard_image(cx: &App) -> Option<PastedImage> {
    let item = cx.read_from_clipboard()?;
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(PastedImage::Bytes(image.clone())),
        ClipboardEntry::ExternalPaths(paths) => {
            paths.paths().first().cloned().map(PastedImage::Path)
        }
        ClipboardEntry::String(_) => None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragUnit {
    Char,
    Word,
    Paragraph,
}

/// How far each level of block quote moves its text in.
const QUOTE_INDENT: Pixels = px(14.);
const QUOTE_BAR_WIDTH: Pixels = px(2.);

struct Snapshot {
    content: String,
    selected: Range<usize>,
}

struct Row {
    start: usize,
    end: usize,
    x0: Pixels,
    top: Pixels,
}

struct Para {
    start: usize,
    end: usize,
    line: std::rc::Rc<WrappedLine>,
    rows: Vec<Row>,
    left: Pixels,
    top: Pixels,
}

struct TextLayout {
    paras: Vec<Para>,
    line_height: Pixels,
    height: Pixels,
    width: Pixels,
    is_placeholder: bool,
}

impl TextLayout {
    fn rows(&self) -> impl Iterator<Item = (&Para, &Row)> {
        self.paras
            .iter()
            .flat_map(|para| para.rows.iter().map(move |row| (para, row)))
    }

    fn x_of(para: &Para, row: &Row, offset: usize) -> Pixels {
        para.line
            .unwrapped_layout
            .x_for_index(offset.saturating_sub(para.start))
            - row.x0
            + para.left
    }

    fn locate(&self, offset: usize) -> Option<(&Para, &Row)> {
        let para = self
            .paras
            .iter()
            .find(|para| offset <= para.end)
            .or(self.paras.last())?;
        let row = para
            .rows
            .iter()
            .find(|row| offset < row.end)
            .or(para.rows.last())?;
        Some((para, row))
    }

    fn position(&self, offset: usize) -> Point<Pixels> {
        match self.locate(offset) {
            Some((para, row)) => point(Self::x_of(para, row, offset), row.top),
            None => point(px(0.), px(0.)),
        }
    }

    fn offset_in_row(para: &Para, row: &Row, x: Pixels, text: &str) -> usize {
        let x = x - para.left + row.x0;
        let index = para.line.unwrapped_layout.closest_index_for_x(x) + para.start;
        let is_last_row = para.rows.last().is_some_and(|last| last.start == row.start);
        let index = if !is_last_row && index >= row.end {
            prev_char(text, row.end)
        } else {
            index.min(row.end)
        };
        floor_char(text, index.max(row.start))
    }

    fn offset_at(&self, position: Point<Pixels>, text: &str) -> usize {
        let hit = self
            .rows()
            .find(|(_, row)| position.y < row.top + self.line_height)
            .or_else(|| self.rows().last());
        match hit {
            Some((para, row)) => Self::offset_in_row(para, row, position.x, text),
            None => 0,
        }
    }
}

fn prev_char(text: &str, offset: usize) -> usize {
    text[..offset]
        .char_indices()
        .next_back()
        .map_or(0, |(ix, _)| ix)
}

fn floor_char(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn is_word(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}

pub struct Editor {
    pub focus: FocusHandle,
    mode: Mode,
    read_only: bool,
    emit_arrows: bool,
    /// Enter breaks the line, leaving saving to the surface around the editor.
    enter_breaks: bool,
    takes_images: bool,
    content: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    spans: Vec<Span>,
    tight: Vec<usize>,
    quotes: Vec<Quote>,
    paragraph_gap: Option<Pixels>,
    layout: Option<TextLayout>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Point<Pixels>,
    autoscroll: bool,
    max_height: Option<Pixels>,
    drag: Option<DragUnit>,
    anchor: Range<usize>,
    goal_x: Option<Pixels>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(EditKind, Instant)>,
    caret_on: bool,
    blink_epoch: usize,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Editor {
    pub fn new(
        mode: Mode,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |this, _, cx| this.restart_blink(cx))
            .detach();
        cx.on_blur(&focus, window, |this, _, cx| {
            this.blink_epoch += 1;
            this.drag = None;
            if this.read_only {
                this.selected = 0..0;
            }
            cx.emit(EditorEvent::Blur);
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && this.focus.is_focused(window) {
                this.restart_blink(cx);
            }
            cx.notify();
        })
        .detach();
        Self {
            focus,
            mode,
            read_only: false,
            emit_arrows: false,
            enter_breaks: false,
            takes_images: false,
            content: String::new(),
            placeholder: placeholder.into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            spans: Vec::new(),
            tight: Vec::new(),
            quotes: Vec::new(),
            paragraph_gap: None,
            layout: None,
            bounds: None,
            scroll: point(px(0.), px(0.)),
            autoscroll: false,
            max_height: None,
            drag: None,
            anchor: 0..0,
            goal_x: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            caret_on: true,
            blink_epoch: 0,
        }
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn emit_arrows(mut self) -> Self {
        self.emit_arrows = true;
        self
    }

    pub fn enter_breaks(mut self) -> Self {
        self.enter_breaks = true;
        self
    }

    pub fn takes_images(mut self) -> Self {
        self.takes_images = true;
        self
    }

    pub fn max_height(mut self, height: Pixels) -> Self {
        self.max_height = Some(height);
        self
    }

    pub fn paragraph_gap(mut self, gap: Pixels) -> Self {
        self.paragraph_gap = Some(gap);
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = self.sanitize(text);
        self.selected = self.content.len()..self.content.len();
        self.reversed = false;
        self.marked = None;
        self.spans.clear();
        self.tight.clear();
        self.quotes.clear();
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.scroll = point(px(0.), px(0.));
        cx.notify();
    }

    /// Shows Markdown already turned into styled text.
    pub fn set_rendered(&mut self, rendered: Rendered, cx: &mut Context<Self>) {
        self.set_text(&rendered.text, cx);
        self.spans = rendered.spans;
        self.tight = rendered.tight;
        self.quotes = rendered.quotes;
    }

    /// Whether this is what the editor shows already, down to its styling.
    pub fn shows(&self, rendered: &Rendered) -> bool {
        self.content == rendered.text
            && self.spans == rendered.spans
            && self.tight == rendered.tight
            && self.quotes == rendered.quotes
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_on = true;
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(530))
                    .await;
                let keep_going = this.update(cx, |this, cx| {
                    if this.blink_epoch != epoch {
                        return false;
                    }
                    this.caret_on = !this.caret_on;
                    cx.notify();
                    true
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn sanitize(&self, text: &str) -> String {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        text.chars()
            .filter_map(|ch| match ch {
                '\n' if self.mode == Mode::Multi => Some('\n'),
                '\n' | '\t' => Some(' '),
                ch if ch.is_control() => None,
                ch => Some(ch),
            })
            .collect()
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn selection_changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.autoscroll = true;
        self.last_edit = None;
        window.notify_text_selection_changed(&self.focus);
        if self.focus.is_focused(window) {
            self.restart_blink(cx);
        }
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        self.goal_x = None;
        self.selection_changed(window, cx);
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        self.goal_x = None;
        self.selection_changed(window, cx);
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(ix, _)| ix)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .graphemes(true)
            .next()
            .map_or(self.content.len(), |grapheme| offset + grapheme.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .rev()
            .find(|(ix, segment)| *ix < offset && is_word(segment))
            .map_or(0, |(ix, _)| ix)
    }

    fn next_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .find(|(ix, segment)| ix + segment.len() > offset && is_word(segment))
            .map_or(self.content.len(), |(ix, segment)| ix + segment.len())
    }

    fn word_range(&self, offset: usize) -> Range<usize> {
        self.content
            .split_word_bound_indices()
            .find(|(ix, segment)| offset < ix + segment.len())
            .map_or(offset..offset, |(ix, segment)| ix..ix + segment.len())
    }

    fn paragraph_start(&self, offset: usize) -> usize {
        self.content[..offset].rfind('\n').map_or(0, |ix| ix + 1)
    }

    fn paragraph_end(&self, offset: usize) -> usize {
        self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |ix| offset + ix)
    }

    fn line_start(&self, offset: usize) -> usize {
        self.layout
            .as_ref()
            .and_then(|layout| layout.locate(offset))
            .map_or(0, |(_, row)| row.start)
    }

    fn line_end(&self, offset: usize) -> usize {
        // The layout of an empty editor holds the placeholder.
        if self.content.is_empty() {
            return 0;
        }
        let Some((para, row)) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.locate(offset))
        else {
            return self.content.len();
        };
        if row.end == para.end {
            row.end
        } else {
            prev_char(&self.content, row.end)
        }
    }

    fn vertical(&mut self, down: bool, from: usize) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(layout) = self.layout.as_ref() else {
            return from;
        };
        let rows: Vec<_> = layout.rows().collect();
        let Some((para, row)) = layout.locate(from) else {
            return from;
        };
        let goal = self
            .goal_x
            .unwrap_or_else(|| TextLayout::x_of(para, row, from));
        let ix = rows
            .iter()
            .position(|(_, candidate)| candidate.start == row.start)
            .unwrap_or(0);
        let target = if down {
            ix.checked_add(1).filter(|ix| *ix < rows.len())
        } else {
            ix.checked_sub(1)
        };
        let offset = match target {
            Some(ix) => TextLayout::offset_in_row(rows[ix].0, rows[ix].1, goal, &self.content),
            None if down => self.content.len(),
            None => 0,
        };
        self.goal_x = Some(goal);
        offset
    }

    fn collapse_or(&self, start_side: bool, moved: impl FnOnce(&Self) -> usize) -> usize {
        match (self.selected.is_empty(), start_side) {
            (true, _) => moved(self),
            (false, true) => self.selected.start,
            (false, false) => self.selected.end,
        }
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.collapse_or(true, |this| this.previous_boundary(this.cursor()));
        self.move_to(offset, window, cx);
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.collapse_or(false, |this| this.next_boundary(this.cursor()));
        self.move_to(offset, window, cx);
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        if self.emit_arrows {
            cx.emit(EditorEvent::Up);
            return;
        }
        let from = if self.selected.is_empty() {
            self.cursor()
        } else {
            self.selected.start
        };
        let goal = self.goal_x;
        let offset = self.vertical(false, from);
        let kept = self.goal_x.or(goal);
        self.move_to(offset, window, cx);
        self.goal_x = kept;
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        if self.emit_arrows {
            cx.emit(EditorEvent::Down);
            return;
        }
        let from = if self.selected.is_empty() {
            self.cursor()
        } else {
            self.selected.end
        };
        let offset = self.vertical(true, from);
        let kept = self.goal_x;
        self.move_to(offset, window, cx);
        self.goal_x = kept;
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor()), window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor()), window, cx);
    }

    fn line_start_action(&mut self, _: &LineStart, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(self.cursor()), window, cx);
    }

    fn line_end_action(&mut self, _: &LineEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor()), window, cx);
    }

    fn paragraph_start_action(
        &mut self,
        _: &ParagraphStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(self.paragraph_start(self.cursor()), window, cx);
    }

    fn paragraph_end_action(
        &mut self,
        _: &ParagraphEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(self.paragraph_end(self.cursor()), window, cx);
    }

    fn doc_start(&mut self, _: &DocStart, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), window, cx);
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical(false, self.cursor());
        let kept = self.goal_x;
        self.select_to(offset, window, cx);
        self.goal_x = kept;
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical(true, self.cursor());
        let kept = self.goal_x;
        self.select_to(offset, window, cx);
        self.goal_x = kept;
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_word(self.cursor()), window, cx);
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_word(self.cursor()), window, cx);
    }

    fn select_line_start(
        &mut self,
        _: &SelectLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.line_start(self.cursor()), window, cx);
    }

    fn select_line_end(&mut self, _: &SelectLineEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.cursor()), window, cx);
    }

    fn select_paragraph_start(
        &mut self,
        _: &SelectParagraphStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.paragraph_start(self.cursor()), window, cx);
    }

    fn select_paragraph_end(
        &mut self,
        _: &SelectParagraphEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.paragraph_end(self.cursor()), window, cx);
    }

    fn select_doc_start(
        &mut self,
        _: &SelectDocStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, window, cx);
    }

    fn select_doc_end(&mut self, _: &SelectDocEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        self.selection_changed(window, cx);
        self.autoscroll = false;
    }

    fn record(&mut self, kind: EditKind) {
        let coalesce = kind != EditKind::Other
            && self.selected.is_empty()
            && self.last_edit.is_some_and(|(last, at)| {
                last == kind && at.elapsed() < Duration::from_millis(1200)
            });
        if !coalesce {
            self.undo.push(Snapshot {
                content: self.content.clone(),
                selected: self.selected.clone(),
            });
        }
        self.redo.clear();
        self.last_edit = Some((kind, Instant::now()));
    }

    fn edited(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.autoscroll = true;
        self.goal_x = None;
        self.reversed = false;
        window.notify_text_input_changed(&self.focus);
        self.restart_blink(cx);
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn splice(&mut self, range: Range<usize>, text: &str, window: &Window, cx: &mut Context<Self>) {
        self.content.replace_range(range.clone(), text);
        let caret = range.start + text.len();
        self.selected = caret..caret;
        self.edited(window, cx);
    }

    fn delete_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let range = if self.selected.is_empty() {
            let cursor = self.cursor();
            cursor.min(offset)..cursor.max(offset)
        } else {
            self.selected.clone()
        };
        if range.is_empty() {
            return;
        }
        self.record(EditKind::Deleting);
        self.splice(range, "", window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(self.previous_boundary(self.cursor()), window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(self.next_boundary(self.cursor()), window, cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to(self.previous_word(self.cursor()), window, cx);
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to(self.next_word(self.cursor()), window, cx);
    }

    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to(self.line_start(self.cursor()), window, cx);
    }

    fn delete_to_paragraph_end(
        &mut self,
        _: &DeleteToParagraphEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to(self.paragraph_end(self.cursor()), window, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            let text = self.content[self.selected.clone()].to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || self.selected.is_empty() {
            return;
        }
        let text = self.content[self.selected.clone()].to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.record(EditKind::Other);
        self.splice(self.selected.clone(), "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.takes_images
            && let Some(image) = clipboard_image(cx)
        {
            cx.emit(EditorEvent::PastedImage(image));
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = self.sanitize(&text);
            self.record(EditKind::Other);
            self.splice(self.selected.clone(), &text, window, cx);
        }
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.undo.pop() {
            let current = Snapshot {
                content: std::mem::replace(&mut self.content, snapshot.content),
                selected: std::mem::replace(&mut self.selected, snapshot.selected),
            };
            self.redo.push(current);
            self.marked = None;
            self.last_edit = None;
            self.edited(window, cx);
        }
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.redo.pop() {
            let current = Snapshot {
                content: std::mem::replace(&mut self.content, snapshot.content),
                selected: std::mem::replace(&mut self.selected, snapshot.selected),
            };
            self.undo.push(current);
            self.marked = None;
            self.last_edit = None;
            self.edited(window, cx);
        }
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.enter_breaks {
            self.insert_newline(&InsertNewline, window, cx);
        } else {
            cx.emit(EditorEvent::Submit);
        }
    }

    fn insert_newline(&mut self, _: &InsertNewline, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Multi && !self.read_only {
            self.record(EditKind::Other);
            self.splice(self.selected.clone(), "\n", window, cx);
        } else {
            cx.emit(EditorEvent::Submit);
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Cancel);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        match (&self.layout, self.bounds) {
            (Some(layout), Some(bounds)) if !layout.is_placeholder => {
                layout.offset_at(position - bounds.origin + self.scroll, &self.content)
            }
            _ => 0,
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.index_for_position(event.position);
        if self.read_only
            && event.click_count == 1
            && let Some(url) = self.link_at(offset)
        {
            cx.open_url(&url);
            cx.stop_propagation();
            return;
        }
        match event.click_count {
            1 => {
                self.drag = Some(DragUnit::Char);
                if event.modifiers.shift {
                    self.select_to(offset, window, cx);
                } else {
                    self.move_to(offset, window, cx);
                }
            }
            2 => {
                self.drag = Some(DragUnit::Word);
                self.anchor = self.word_range(offset);
                self.selected = self.anchor.clone();
                self.reversed = false;
                self.selection_changed(window, cx);
            }
            _ => {
                self.drag = Some(DragUnit::Paragraph);
                self.anchor = self.paragraph_start(offset)..self.paragraph_end(offset);
                self.selected = self.anchor.clone();
                self.reversed = false;
                self.selection_changed(window, cx);
            }
        }
    }

    fn link_at(&self, offset: usize) -> Option<String> {
        self.spans
            .iter()
            .find(|span| span.range.contains(&offset))
            .and_then(|span| span.style.link.clone())
    }

    fn drag_to(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let offset = self.index_for_position(position);
        let span = match self.drag {
            Some(DragUnit::Char) => return self.select_to(offset, window, cx),
            Some(DragUnit::Word) => self.word_range(offset),
            Some(DragUnit::Paragraph) => self.paragraph_start(offset)..self.paragraph_end(offset),
            None => return,
        };
        self.reversed = span.start < self.anchor.start;
        self.selected = self.anchor.start.min(span.start)..self.anchor.end.max(span.end);
        self.selection_changed(window, cx);
    }

    fn on_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = &self.layout else { return };
        let Some(bounds) = self.bounds else { return };
        let delta = event.delta.pixel_delta(window.line_height());
        match self.mode {
            Mode::Single => {
                let max = (layout.width - bounds.size.width + px(2.)).max(px(0.));
                self.scroll.x = (self.scroll.x - delta.x).clamp(px(0.), max);
            }
            _ => {
                let max = (layout.height - bounds.size.height).max(px(0.));
                if max > px(0.) {
                    cx.stop_propagation();
                }
                self.scroll.y = (self.scroll.y - delta.y).clamp(px(0.), max);
            }
        }
        cx.notify();
    }

    fn runs(&self, range: Range<usize>, style: &TextStyle, color: Hsla, cx: &App) -> Vec<TextRun> {
        let mut cuts = vec![range.start, range.end];
        let inside = |offset: usize| offset > range.start && offset < range.end;
        let spans = self.spans.iter().map(|span| &span.range);
        for span in self.marked.iter().chain(spans) {
            cuts.extend([span.start, span.end].into_iter().filter(|o| inside(*o)));
        }
        cuts.sort_unstable();
        cuts.dedup();
        let theme = theme(cx);
        cuts.windows(2)
            .map(|pair| {
                let (start, end) = (pair[0], pair[1]);
                let contains = |span: &Range<usize>| span.start <= start && end <= span.end;
                let span = self.spans.iter().find(|span| contains(&span.range));
                let marks = span.map(|span| span.style.clone()).unwrap_or_default();
                let mut font = style.font();
                if marks.bold {
                    font.weight = FontWeight::SEMIBOLD;
                }
                if marks.italic {
                    font.style = gpui::FontStyle::Italic;
                }
                if marks.code {
                    font.family = crate::kit::MONO.into();
                }
                let link = marks.link.is_some();
                let tint = if link {
                    theme.accent
                } else if marks.dim {
                    theme.muted
                } else {
                    color
                };
                let underline = self.marked.as_ref().is_some_and(contains) || link;
                TextRun {
                    len: end - start,
                    font,
                    color: tint,
                    background_color: marks.code.then_some(theme.raised),
                    underline: underline.then_some(UnderlineStyle {
                        color: Some(tint),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    strikethrough: marks.strike.then_some(gpui::StrikethroughStyle {
                        color: Some(tint),
                        thickness: px(1.),
                    }),
                }
            })
            .collect()
    }

    fn shape(
        &self,
        style: &TextStyle,
        line_height: Pixels,
        wrap_width: Option<Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> TextLayout {
        let is_placeholder = self.content.is_empty();
        let theme = theme(cx);
        let (text, color): (&str, Hsla) = if is_placeholder {
            (self.placeholder.as_ref(), theme.faint)
        } else {
            (self.content.as_str(), style.color)
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let wrap_width = if self.mode == Mode::Single {
            None
        } else {
            wrap_width
        };
        let mut paras = Vec::new();
        let mut top = px(0.);
        let mut width = px(0.);
        let mut start = 0;
        let sources: Vec<&str> = text.split('\n').collect();
        for (ix, source) in sources.iter().enumerate() {
            let end = start + source.len();
            let depth = self.quotes.iter().filter(|quote| quote.lines.contains(&ix));
            let left = QUOTE_INDENT * depth.count() as f32;
            let runs = if is_placeholder {
                let mut runs = self.runs(0..0, style, color, cx);
                runs.push(TextRun {
                    len: source.len(),
                    font: style.font(),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
                runs
            } else {
                self.runs(start..end, style, color, cx)
            };
            let line = window
                .text_system()
                .shape_text(
                    SharedString::from(source.to_string()),
                    font_size,
                    &runs,
                    wrap_width.map(|width| (width - left).max(px(0.))),
                    None,
                )
                .ok()
                .and_then(|mut lines| {
                    if lines.is_empty() {
                        None
                    } else {
                        Some(lines.swap_remove(0))
                    }
                });
            let Some(line) = line else {
                start = end + 1;
                continue;
            };
            let layout = &**line;
            let mut boundaries: Vec<usize> = layout
                .wrap_boundaries
                .iter()
                .map(|b| layout.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix].index)
                .collect();
            boundaries.push(source.len());
            let mut rows = Vec::with_capacity(boundaries.len());
            let mut row_start = 0;
            let para_top = top;
            for boundary in boundaries {
                let x0 = layout.unwrapped_layout.x_for_index(row_start);
                let right = layout.unwrapped_layout.x_for_index(boundary) - x0;
                width = width.max(left + right);
                rows.push(Row {
                    start: start + row_start,
                    end: start + boundary,
                    x0,
                    top,
                });
                top += line_height;
                row_start = boundary;
            }
            if let Some(gap) = self.paragraph_gap
                && ix + 1 < sources.len()
                && !self.tight.contains(&ix)
            {
                top += gap;
            }
            paras.push(Para {
                start,
                end,
                line: line.into(),
                rows,
                left,
                top: para_top,
            });
            start = end + 1;
        }
        TextLayout {
            paras,
            line_height,
            height: top.max(line_height),
            width,
            is_placeholder,
        }
    }

    fn caret_bounds(&self, offset: usize) -> Option<(Point<Pixels>, Pixels)> {
        let layout = self.layout.as_ref()?;
        Some((layout.position(offset), layout.line_height))
    }

    fn scroll_caret_into_view(&mut self, size: Size<Pixels>) {
        let Some(layout) = &self.layout else { return };
        let caret = layout.position(self.cursor());
        let (width, height, line_height) = (layout.width, layout.height, layout.line_height);
        if std::mem::take(&mut self.autoscroll) {
            if self.mode == Mode::Single {
                if caret.x < self.scroll.x {
                    self.scroll.x = caret.x;
                } else if caret.x + px(2.) > self.scroll.x + size.width {
                    self.scroll.x = caret.x + px(2.) - size.width;
                }
            } else if caret.y < self.scroll.y {
                self.scroll.y = caret.y;
            } else if caret.y + line_height > self.scroll.y + size.height {
                self.scroll.y = caret.y + line_height - size.height;
            }
        }
        self.scroll.x = self
            .scroll
            .x
            .clamp(px(0.), (width - size.width + px(2.)).max(px(0.)));
        self.scroll.y = self
            .scroll
            .y
            .clamp(px(0.), (height - size.height).max(px(0.)));
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..floor_char(&self.content, offset)]
            .encode_utf16()
            .count()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected),
            reversed: self.reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = self.range_from_utf16(&range_utf16);
        self.reversed = false;
        self.selection_changed(window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        let text = self.sanitize(new_text);
        if self.marked.take().is_none() {
            self.record(EditKind::Typing);
        }
        self.splice(range, &text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        if self.marked.is_none() {
            self.record(EditKind::Other);
        }
        self.content.replace_range(range.clone(), new_text);
        self.marked = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let start_utf16 = self.offset_to_utf16(range.start);
        let length_utf16 = new_text.encode_utf16().count();
        self.selected = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(start_utf16 + selected.start.min(length_utf16)
                        ..start_utf16 + selected.end.min(length_utf16)),
                )
            })
            .unwrap_or(range.start + new_text.len()..range.start + new_text.len());
        self.edited(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (start, line_height) = self.caret_bounds(range.start)?;
        let (end, _) = self.caret_bounds(range.end)?;
        let origin = bounds.origin + start - self.scroll;
        let width = if start.y == end.y {
            end.x - start.x
        } else {
            px(1.)
        };
        Some(Bounds::new(origin, size(width, line_height)))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.index_for_position(point);
        Some(self.offset_to_utf16(offset))
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .key_context("TextInput")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_paragraph_end))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::line_start_action))
            .on_action(cx.listener(Self::line_end_action))
            .on_action(cx.listener(Self::paragraph_start_action))
            .on_action(cx.listener(Self::paragraph_end_action))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::select_paragraph_start))
            .on_action(cx.listener(Self::select_paragraph_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(EditorElement {
                editor: cx.entity(),
            })
    }
}

struct EditorElement {
    editor: Entity<Editor>,
}

struct Prepaint {
    quote_bars: Vec<PaintQuad>,
    selections: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    paints: Vec<(std::rc::Rc<WrappedLine>, Point<Pixels>)>,
    line_height: Pixels,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

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
        _: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let text_style = window.text_style();
        let line_height = window.line_height();
        let editor = self.editor.clone();
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, cx| {
                let width = known.width.or(match available.width {
                    gpui::AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let editor = editor.read(cx);
                let mut height = if editor.mode == Mode::Single {
                    line_height
                } else {
                    editor
                        .shape(&text_style, line_height, width, window, cx)
                        .height
                };
                if let Some(max) = editor.max_height {
                    height = height.min(max);
                }
                size(width.unwrap_or(px(0.)), height)
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let line_height = window.line_height();
        let layout =
            self.editor
                .read(cx)
                .shape(&style, line_height, Some(bounds.size.width), window, cx);
        self.editor.update(cx, |editor, _| {
            editor.layout = Some(layout);
            editor.bounds = Some(bounds);
            editor.scroll_caret_into_view(bounds.size);
        });

        let colors = theme(cx);
        let active = window.is_window_active();
        let editor = self.editor.read(cx);
        let focused = editor.focus.is_focused(window);
        let origin = bounds.origin - editor.scroll;
        let layout = editor.layout.as_ref().expect("layout was just stored");

        let quote_bars = editor
            .quotes
            .iter()
            .filter_map(|quote| {
                let paras = layout.paras.get(quote.lines.clone())?;
                let top = paras.first()?.top;
                let bottom = paras.last()?.rows.last()?.top + line_height;
                let left = QUOTE_INDENT * quote.depth as f32;
                Some(fill(
                    Bounds::new(
                        origin + point(left, top),
                        size(QUOTE_BAR_WIDTH, bottom - top),
                    ),
                    colors.line2,
                ))
            })
            .collect();

        let mut selections = Vec::new();
        if !editor.selected.is_empty() {
            let color = if focused && active {
                colors.accent.opacity(0.32)
            } else {
                colors.faint.opacity(0.28)
            };
            for (para, row) in layout.rows() {
                let start = editor.selected.start.max(row.start);
                let end = editor.selected.end.min(row.end);
                let crosses_newline = editor.selected.end > para.end && row.end == para.end;
                if start > end || (start == end && !crosses_newline) {
                    continue;
                }
                let left = TextLayout::x_of(para, row, start);
                let mut right = TextLayout::x_of(para, row, end);
                if crosses_newline {
                    right += px(5.);
                }
                selections.push(fill(
                    Bounds::from_corners(
                        origin + point(left, row.top),
                        origin + point(right, row.top + line_height),
                    ),
                    color,
                ));
            }
        }

        let caret = (focused
            && active
            && !editor.read_only
            && editor.caret_on
            && editor.selected.is_empty())
        .then(|| {
            let position = layout.position(editor.cursor());
            fill(
                Bounds::new(origin + position, size(px(1.), line_height)),
                colors.ink,
            )
        });

        let paints = layout
            .paras
            .iter()
            .map(|para| (para.line.clone(), origin + point(para.left, para.top)))
            .collect();

        Prepaint {
            quote_bars,
            selections,
            caret,
            paints,
            line_height,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let editor = self.editor.read(cx);
        let focus = editor.focus.clone();
        let read_only = editor.read_only;
        let dragging = editor.drag.is_some();
        if !read_only {
            window.handle_input(
                &focus,
                ElementInputHandler::new(bounds, self.editor.clone()),
                cx,
            );
        }
        let mask_bounds = Bounds::from_corners(
            bounds.origin - point(px(1.), px(0.)),
            bounds.bottom_right() + point(px(2.), px(0.)),
        );
        let line_height = prepaint.line_height;
        let quote_bars = std::mem::take(&mut prepaint.quote_bars);
        let selections = std::mem::take(&mut prepaint.selections);
        let paints = std::mem::take(&mut prepaint.paints);
        let caret = prepaint.caret.take();
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: mask_bounds,
            }),
            |window| {
                for quad in quote_bars.into_iter().chain(selections) {
                    window.paint_quad(quad);
                }
                for (line, origin) in paints {
                    line.paint(origin, line_height, gpui::TextAlign::Left, None, window, cx)
                        .ok();
                }
                if let Some(caret) = caret {
                    window.paint_quad(caret);
                }
            },
        );
        if dragging {
            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase.bubble() && event.dragging() {
                    editor.update(cx, |editor, cx| editor.drag_to(event.position, window, cx));
                }
            });
            let editor = self.editor.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase.bubble() {
                    editor.update(cx, |editor, cx| {
                        editor.drag = None;
                        cx.notify();
                    });
                }
            });
        }
    }
}

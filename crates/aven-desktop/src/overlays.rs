//! Surfaces above the window: menus and the toast.
use std::rc::Rc;
use std::time::Duration;

use accesskit::Role;
use aven_core::choices::{TaskPriority, TaskStatus};
use chrono::NaiveDate;
use gpui::{
    Action, Animation, AnimationExt, App, Bounds, Context, Entity, FocusHandle, FontWeight,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollHandle, SharedString,
    Subscription, Window, div, point, prelude::*, relative,
};

use crate::input::{Editor, EditorEvent, Mode};
use crate::kit::*;
use crate::schedule::{CALENDAR_HEIGHT, Calendar};
use crate::session::ToastAction;
use crate::shell::Root;
use crate::theme::theme;
use aven_core::labels::normalize_label;

pub const TOAST_DURATION: Duration = Duration::from_secs(5);
const MENU_ENTER: Duration = Duration::from_millis(140);
const MENU_WIDTH: f32 = 280.;
const PLAIN_MENU_WIDTH: f32 = 236.;
const WIDE_MENU_WIDTH: f32 = 620.;
const MENU_PADDING: f32 = 5.;
const MENU_HEADER_HEIGHT: f32 = 28.;
const MENU_NOTE_HEIGHT: f32 = 52.;

pub type Run = Rc<dyn Fn(&mut Root, &mut Window, &mut Context<Root>)>;

#[derive(Clone)]
pub enum Mark {
    None,
    Status(TaskStatus),
    Priority(TaskPriority),
    Icon(&'static str),
    Project(String),
}

#[derive(Clone)]
pub struct Entry {
    pub head: bool,
    pub label: String,
    pub hint: String,
    pub mark: Mark,
    pub checked: bool,
    pub off: bool,
    pub sep: bool,
    pub danger: bool,
    /// Checking one entry of a group unchecks the rest.
    pub group: u8,
    /// Shown under the list while the entry is highlighted.
    pub note: Option<String>,
    /// Picking it swaps the list for the menu's calendar.
    calendar: bool,
    key: Option<char>,
    run: Option<Run>,
}

impl Entry {
    pub fn new(
        label: impl Into<String>,
        run: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
    ) -> Self {
        Self {
            head: false,
            label: label.into(),
            hint: String::new(),
            mark: Mark::None,
            checked: false,
            off: false,
            sep: false,
            danger: false,
            group: 0,
            note: None,
            calendar: false,
            key: None,
            run: Some(Rc::new(run)),
        }
    }

    pub fn head(label: &str) -> Self {
        Self {
            head: true,
            run: None,
            ..Self::new(label, |_, _, _| {})
        }
    }

    /// An entry that dispatches a command, so it does what its key does.
    pub fn action(label: impl Into<String>, hint: &str, action: impl Action) -> Self {
        Self::new(label, move |_, window, cx| {
            window.dispatch_action(action.boxed_clone(), cx);
        })
        .hint(hint)
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn mark(mut self, mark: Mark) -> Self {
        self.mark = mark;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn off(mut self, off: bool) -> Self {
        self.off = off;
        self
    }

    pub fn sep(mut self) -> Self {
        self.sep = true;
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn group(mut self, group: u8) -> Self {
        self.group = group;
        self
    }

    pub fn note(mut self, note: Option<String>) -> Self {
        self.note = note;
        self
    }

    pub fn calendar(mut self) -> Self {
        self.calendar = true;
        self
    }

    /// The letter that picks this entry in a choice menu.
    pub fn key(mut self, key: char) -> Self {
        self.key = Some(key);
        self.hint = key.to_string();
        self
    }
}

/// Makes the entry a create line stands for.
pub type Create = Rc<dyn Fn(String) -> Entry>;

/// Makes sense of typed text as an entry of its own, if it can.
pub type Read = Rc<dyn Fn(&str) -> Option<Entry>>;

/// Names what a menu sets, and the menu Tab swaps it for.
pub struct MenuHeader {
    pub title: &'static str,
    pub subject: String,
    pub other: &'static str,
    pub swap: Run,
}

#[derive(Default)]
pub struct MenuOptions {
    pub wide: bool,
    pub plain: bool,
    pub multi: bool,
    pub create: Option<Create>,
    /// The title of a menu of fixed choices, which takes letters instead of a search.
    pub choice: Option<String>,
    pub read: Option<Read>,
    /// What the empty list says about the text that left nothing.
    pub unread: Option<fn(&str) -> String>,
    pub header: Option<MenuHeader>,
    pub calendar: Option<Calendar>,
}

#[derive(Clone, Copy, PartialEq)]
enum Shown {
    Item(usize),
    Create,
    Read,
}

pub struct Menu {
    entries: Vec<Entry>,
    options: MenuOptions,
    input: Entity<Editor>,
    index: usize,
    anchor: Bounds<Pixels>,
    prior: Option<FocusHandle>,
    run: usize,
    scroll: ScrollHandle,
    choice_focus: FocusHandle,
    calendar_open: bool,
    _subscription: Subscription,
}

/// How closely typed text matches a menu entry, tightest first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Closeness {
    Exact,
    Prefix,
    /// Unbroken, starting at a word of the label such as `agent` in `slack-agent`.
    Segment,
    /// Unbroken, starting partway through a word of the label.
    Inside,
    /// Unbroken in the hint only.
    Hint,
    /// Letters in order, spread over `span` letters from `start` in the label.
    Scattered {
        span: usize,
        start: usize,
    },
}

/// How `word` sits unbroken in `label`, if it does.
fn unbroken(word: &str, label: &str) -> Option<Closeness> {
    let starts_segment = |(at, _): (usize, char)| {
        !label[..at].ends_with(char::is_alphanumeric) && label[at..].starts_with(word)
    };
    if label == word {
        Some(Closeness::Exact)
    } else if label.starts_with(word) {
        Some(Closeness::Prefix)
    } else if label.char_indices().any(starts_segment) {
        Some(Closeness::Segment)
    } else if label.contains(word) {
        Some(Closeness::Inside)
    } else {
        None
    }
}

/// The fewest letters of `text` that hold all of `word`'s letters in order, and
/// where the earliest such stretch starts.
fn shortest_span(word: &str, text: &str) -> Option<(usize, usize)> {
    let first = word.chars().next()?;
    text.char_indices()
        .filter(|(_, letter)| *letter == first)
        .filter_map(|(start, _)| {
            let mut letters = text[start..].chars().enumerate();
            let mut span = 0;
            let found = word.chars().all(|wanted| {
                letters.any(|(ix, letter)| {
                    span = ix + 1;
                    letter == wanted
                })
            });
            found.then_some((span, start))
        })
        .min()
}

/// How closely a lowercase `query` matches a lowercase entry, if at all. The
/// query as a whole counts first; otherwise each word must match and the
/// loosest word decides. Loose matching skips hints, where long text would
/// match almost anything.
fn closeness(query: &str, label: &str, hint: &str) -> Option<Closeness> {
    let phrase = unbroken(query, label);
    if phrase.is_some() {
        return phrase;
    }
    let mut words = query.split_whitespace();
    let whole: Option<Vec<Closeness>> = words
        .clone()
        .map(|word| unbroken(word, label).or(hint.contains(word).then_some(Closeness::Hint)))
        .collect();
    if let Some(whole) = whole {
        return whole.into_iter().max();
    }
    let (span, start) = words.try_fold((0, 0), |(span, start), word| {
        let (word_span, word_start) = shortest_span(word, label)?;
        Some((span + word_span, start + word_start))
    })?;
    Some(Closeness::Scattered { span, start })
}

/// The scroll offset that shows `row` inside `view` with the list's padding around it.
fn revealed_offset(row: Bounds<Pixels>, view: Bounds<Pixels>, offset: Pixels) -> Pixels {
    let padding = px(MENU_PADDING);
    if row.top() + offset < view.top() + padding {
        view.top() + padding - row.top()
    } else if row.bottom() + offset > view.bottom() - padding {
        view.bottom() - padding - row.bottom()
    } else {
        offset
    }
}

impl Menu {
    fn query(&self, cx: &App) -> String {
        self.input.read(cx).text().to_lowercase()
    }

    /// What the typed text reads as, if the menu reads text.
    fn read(&self, cx: &App) -> Option<Entry> {
        let query = self.query(cx);
        let query = query.trim();
        let read = self.options.read.as_ref().filter(|_| !query.is_empty())?;
        read(query)
    }

    /// The calendar, while it stands in for the list. Typing returns to the list.
    fn calendar(&self, cx: &App) -> Option<&Calendar> {
        let calendar = self.options.calendar.as_ref();
        calendar.filter(|_| self.calendar_open && self.query(cx).trim().is_empty())
    }

    /// The entries the typed text leaves, closest first and otherwise in order.
    /// What it reads as leads, in place of the entries that say the same.
    fn shown(&self, cx: &App) -> Vec<Shown> {
        let query = self.query(cx);
        let query = query.trim();
        let read = self.read(cx);
        let said = |entry: &Entry| {
            let said = read.as_ref();
            said.is_some_and(|read| entry.label == read.label || entry.hint == read.label)
        };
        let mut ranked: Vec<(Closeness, usize)> = (0..self.entries.len())
            .filter_map(|ix| {
                let entry = &self.entries[ix];
                if query.is_empty() {
                    return Some((Closeness::Exact, ix));
                }
                if entry.head || said(entry) {
                    return None;
                }
                let label = entry.label.to_lowercase();
                let found = closeness(query, &label, &entry.hint.to_lowercase());
                found.map(|closeness| (closeness, ix))
            })
            .collect();
        ranked.sort_by_key(|(closeness, _)| *closeness);
        let mut shown: Vec<Shown> = ranked.into_iter().map(|(_, ix)| Shown::Item(ix)).collect();
        let exact = self
            .entries
            .iter()
            .any(|entry| entry.label.to_lowercase() == query);
        if self.options.create.is_some() && !query.is_empty() && !exact {
            shown.push(Shown::Create);
        }
        if read.is_some() {
            shown.insert(0, Shown::Read);
        }
        shown
    }

    /// The shown lines that can be picked, in order.
    fn picks(&self, cx: &App) -> Vec<Shown> {
        let can_pick = |line: &Shown| match line {
            Shown::Item(ix) => !self.entries[*ix].head && !self.entries[*ix].off,
            Shown::Create | Shown::Read => true,
        };
        self.shown(cx).into_iter().filter(can_pick).collect()
    }
}

impl Root {
    pub(crate) fn open_menu(
        &mut self,
        entries: Vec<Entry>,
        placeholder: &'static str,
        anchor: Bounds<Pixels>,
        options: MenuOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_some() {
            return;
        }
        let input = cx.new(|cx| Editor::new(Mode::Single, placeholder, window, cx).emit_arrows());
        let subscription = cx.subscribe_in(&input, window, Self::on_menu_event);
        let focus = input.read(cx).focus.clone();
        self.menu = Some(Menu {
            entries,
            index: if options.plain { usize::MAX } else { 0 },
            options,
            choice_focus: cx.focus_handle(),
            calendar_open: false,
            run: self.session.motion.next_run(),
            input,
            anchor,
            prior: window.focused(cx),
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        });
        if let Some(menu) = self
            .menu
            .as_mut()
            .filter(|menu| menu.options.choice.is_some())
        {
            let entries = &menu.entries;
            let current = menu
                .picks(cx)
                .iter()
                .position(|line| matches!(line, Shown::Item(ix) if entries[*ix].checked));
            menu.index = current.unwrap_or(0);
            let focus = menu.choice_focus.clone();
            window.focus(&focus, cx);
        } else {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.take() {
            let focus = menu.prior.unwrap_or_else(|| self.focus.clone());
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// Swaps the open menu for the one `open` opens, without animating it in again.
    pub(crate) fn replace_menu(
        &mut self,
        open: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let run = self.menu.as_ref().map(|menu| menu.run);
        self.close_menu(window, cx);
        open(self, window, cx);
        if let Some((menu, run)) = self.menu.as_mut().zip(run) {
            menu.run = run;
        }
    }

    /// The labels checked in the open menu, for a pick to act on.
    pub(crate) fn menu_checked(&self) -> Vec<String> {
        let entries = self.menu.iter().flat_map(|menu| &menu.entries);
        entries
            .filter(|entry| entry.checked)
            .map(|entry| entry.label.clone())
            .collect()
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let Some(line) = menu.picks(cx).get(index).copied() else {
            return;
        };
        let ix = match line {
            Shown::Item(ix) => ix,
            Shown::Create => {
                let label = normalize_label(&menu.query(cx));
                let create = menu.options.create.clone();
                let Some(entry) = create.map(|create| create(label)) else {
                    return;
                };
                menu.entries.push(entry);
                menu.entries.len() - 1
            }
            Shown::Read => {
                let Some(entry) = menu.read(cx) else {
                    return;
                };
                menu.entries.push(entry);
                menu.entries.len() - 1
            }
        };
        self.activate(ix, window, cx);
    }

    /// Runs the entry at `ix`, however it was chosen.
    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        if menu.entries[ix].calendar {
            menu.calendar_open = true;
            let input = menu.input.clone();
            input.update(cx, |input, cx| input.set_text("", cx));
            cx.notify();
            return;
        }
        let run = menu.entries[ix].run.clone();
        if menu.options.multi {
            let checked = !menu.entries[ix].checked;
            let group = menu.entries[ix].group;
            if group > 0 {
                for entry in &mut menu.entries {
                    if entry.group == group {
                        entry.checked = false;
                    }
                }
            }
            menu.entries[ix].checked = checked;
            let input = menu.input.clone();
            input.update(cx, |input, cx| input.set_text("", cx));
        } else {
            self.close_menu(window, cx);
        }
        if let Some(run) = run {
            run(self, window, cx);
        }
        cx.notify();
    }

    /// The open menu's calendar, for its controls to move.
    pub(crate) fn menu_calendar(&mut self) -> Option<&mut Calendar> {
        self.menu.as_mut()?.options.calendar.as_mut()
    }

    fn step_menu_calendar(&mut self, days: i64, cx: &mut Context<Self>) {
        if let Some(calendar) = self.menu_calendar() {
            calendar.move_days(days);
        }
        cx.notify();
    }

    pub(crate) fn pick_menu_day(
        &mut self,
        day: NaiveDate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pick = self.menu_calendar().map(|calendar| calendar.pick());
        self.close_menu(window, cx);
        if let Some(pick) = pick {
            pick(self, day);
        }
        cx.notify();
    }

    fn on_menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "tab" {
            return;
        }
        let header = self
            .menu
            .as_ref()
            .and_then(|menu| menu.options.header.as_ref());
        if let Some(swap) = header.map(|header| header.swap.clone()) {
            cx.stop_propagation();
            swap(self, window, cx);
        }
    }

    fn on_menu_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        if let Some(day) = menu.calendar(cx).map(|calendar| calendar.day) {
            match event {
                EditorEvent::Up => return self.step_menu_calendar(-7, cx),
                EditorEvent::Down => return self.step_menu_calendar(7, cx),
                EditorEvent::Left => return self.step_menu_calendar(-1, cx),
                EditorEvent::Right => return self.step_menu_calendar(1, cx),
                EditorEvent::Submit => return self.pick_menu_day(day, window, cx),
                _ => {}
            }
        }
        let count = menu.picks(cx).len();
        let index = if menu.index == usize::MAX {
            None
        } else {
            Some(menu.index)
        };
        match event {
            EditorEvent::Changed => self.set_menu_index(0, cx),
            EditorEvent::Up => {
                self.set_menu_index(index.map_or(0, |ix| ix.saturating_sub(1)), cx);
            }
            EditorEvent::Down => {
                let next = index.map_or(0, |ix| (ix + 1).min(count.saturating_sub(1)));
                self.set_menu_index(next, cx);
            }
            EditorEvent::Submit => self.pick(index.unwrap_or(0), window, cx),
            EditorEvent::Cancel => self.close_menu(window, cx),
            _ => {}
        }
    }

    fn on_choice_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let stroke = &event.keystroke;
        if stroke.modifiers.platform || stroke.modifiers.control {
            return;
        }
        let index = menu.index;
        let last = menu.picks(cx).len().saturating_sub(1);
        match stroke.key.as_str() {
            "escape" => self.close_menu(window, cx),
            "enter" => self.pick(index, window, cx),
            "up" => self.set_menu_index(index.saturating_sub(1), cx),
            "down" => self.set_menu_index((index + 1).min(last), cx),
            _ => {
                let typed = stroke
                    .key_char
                    .as_deref()
                    .and_then(|text| text.chars().next());
                let entry = typed.and_then(|typed| {
                    let keyed = |entry: &Entry| entry.key == Some(typed) && !entry.off;
                    menu.entries.iter().position(keyed)
                });
                if let Some(ix) = entry {
                    self.activate(ix, window, cx);
                }
            }
        }
    }

    fn set_menu_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut() {
            menu.index = index;
            let picks = menu.picks(cx);
            let line = picks.get(index).copied();
            let shown = menu.shown(cx);
            let position = shown.iter().position(|shown| Some(*shown) == line);
            if index == 0 {
                // The list may have just been filtered, so old row bounds can't be trusted.
                menu.scroll.set_offset(point(px(0.), px(0.)));
            } else if let Some(position) = position {
                // A separator is a child of the list too.
                let typing = !menu.query(cx).trim().is_empty();
                let separators = shown[..=position]
                    .iter()
                    .filter(|line| {
                        matches!(line, Shown::Item(item) if menu.entries[*item].sep && !typing)
                    })
                    .count();
                let child = position + separators;
                if let Some(row) = menu.scroll.bounds_for_item(child) {
                    let offset = menu.scroll.offset();
                    let y = revealed_offset(row, menu.scroll.bounds(), offset.y);
                    menu.scroll.set_offset(point(offset.x, y));
                }
            }
            cx.notify();
        }
    }

    pub(crate) fn render_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.menu.as_ref()?;
        let t = theme(cx);
        let (wide, plain, multi) = (menu.options.wide, menu.options.plain, menu.options.multi);
        let query = menu.query(cx);
        let typing = !query.trim().is_empty();
        let shown = menu.shown(cx);
        let picks = menu.picks(cx);
        let width = if wide {
            WIDE_MENU_WIDTH
        } else if plain {
            PLAIN_MENU_WIDTH
        } else {
            MENU_WIDTH
        };
        let viewport = window.viewport_size();
        let row = if plain { 28. } else { 30. };
        let choice = menu.options.choice.clone();
        let input_height = match (wide, plain && !typing) {
            _ if choice.is_some() => 34.,
            (true, _) => 48.,
            (_, true) => 0.,
            _ => 38.,
        };
        let header = menu.options.header.as_ref();
        let header_height = header.map_or(0., |_| MENU_HEADER_HEIGHT);
        let calendar = menu.calendar(cx);
        let typed = menu.read(cx);
        let entry_of = |line: &Shown| match line {
            Shown::Item(item) => menu.entries.get(*item),
            Shown::Create => None,
            Shown::Read => typed.as_ref(),
        };
        let note = picks
            .get(menu.index)
            .and_then(entry_of)
            .and_then(|entry| entry.note.clone())
            .filter(|_| calendar.is_none());
        let body: f32 = match calendar {
            Some(_) => CALENDAR_HEIGHT - 10.,
            None => shown.len().max(1) as f32 * row,
        };
        let max_height = if wide {
            470.
        } else if plain {
            f32::MAX
        } else if header.is_some() {
            400.
        } else {
            340.
        };
        let note_height = note.as_ref().map_or(0., |_| MENU_NOTE_HEIGHT);
        let height = (header_height + input_height + 10. + body + note_height).min(max_height);
        let (left, top) = if wide {
            ((viewport.width - px(width)) / 2., viewport.height * 0.13)
        } else {
            let left = menu
                .anchor
                .left()
                .min(viewport.width - px(width + 12.))
                .max(px(12.));
            let below = menu.anchor.bottom() + px(4.);
            let top = if below + px(height) > viewport.height - px(12.)
                && menu.anchor.top() - px(height + 4.) >= px(12.)
            {
                menu.anchor.top() - px(height + 4.)
            } else {
                below
            };
            (
                left,
                top.min(viewport.height - px(height + 12.)).max(px(12.)),
            )
        };
        let mut rows = div()
            .id("menu-rows")
            .flex_1()
            .min_h_0()
            .p(px(MENU_PADDING))
            .overflow_y_scroll()
            .track_scroll(&menu.scroll);
        for (ix, line) in shown.iter().enumerate() {
            let pick = picks.iter().position(|one| one == line);
            let on = pick.is_some() && pick == Some(menu.index);
            let created;
            let (label, entry): (&str, Option<&Entry>) = match line {
                Shown::Item(_) | Shown::Read => {
                    let entry = entry_of(line);
                    (entry.map_or("", |entry| entry.label.as_str()), entry)
                }
                Shown::Create => {
                    created = format!("+ create \"{}\"", normalize_label(&query));
                    (created.as_str(), None)
                }
            };
            if entry.is_some_and(|entry| entry.head) {
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .min_h(px(26.))
                        .px(px(8.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.faint)
                        .child(label.to_string()),
                );
                continue;
            }
            let danger = entry.is_some_and(|entry| entry.danger);
            let off = entry.is_some_and(|entry| entry.off);
            if entry.is_some_and(|entry| entry.sep) && !typing {
                rows = rows.child(div().h(px(1.)).my(px(5.)).mx(px(-5.)).bg(t.line));
            }
            let mark = entry.map_or(Mark::None, |entry| entry.mark.clone());
            let color = if danger { t.bad } else { t.muted };
            let mark = match mark {
                Mark::None if plain => Some(div().flex_none().size(px(16.)).into_any_element()),
                Mark::None => None,
                Mark::Status(status) => Some(status_glyph(status, &t).into_any_element()),
                Mark::Priority(priority) => Some(priority_icon(priority, &t).into_any_element()),
                Mark::Icon(path) => Some(icon(path, color).into_any_element()),
                Mark::Project(key) => Some(project_dot(&key, &t).into_any_element()),
            };
            let hint = entry.map(|entry| entry.hint.clone()).unwrap_or_default();
            rows = rows.child(
                div()
                    .id(("menu-item", ix))
                    .role(Role::MenuItem)
                    .aria_label(label.to_string())
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .min_h(px(row))
                    .px(px(8.))
                    .rounded(px(6.))
                    .text_color(if danger {
                        t.bad
                    } else if off {
                        t.faint
                    } else if on {
                        t.ink
                    } else {
                        t.body
                    })
                    .when(on, |row| row.bg(t.pick()))
                    .when_some(pick, |row, pick| {
                        row.on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                            if this.menu.as_ref().is_some_and(|menu| menu.index != pick) {
                                if let Some(menu) = this.menu.as_mut() {
                                    menu.index = pick;
                                }
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.pick(pick, window, cx);
                            },
                        ))
                    })
                    .when(multi || choice.is_some() || header.is_some(), |row| {
                        let checked = entry.is_some_and(|entry| entry.checked);
                        row.child(
                            div()
                                .flex_none()
                                .w(px(12.))
                                .text_color(t.accent)
                                .child(if checked { "✓" } else { "" }),
                        )
                    })
                    .children(mark)
                    .child(div().min_w_0().truncate().child(label.to_string()))
                    .when(!hint.is_empty(), |row| {
                        row.child(
                            small(12.)
                                .ml_auto()
                                .flex_none()
                                .text_color(t.faint)
                                .child(hint),
                        )
                    }),
            );
        }
        if shown.is_empty() {
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .min_h(px(row))
                    .px(px(8.))
                    .text_color(t.body)
                    .child(menu.options.unread.map_or_else(
                        || "No results".to_string(),
                        |unread| unread(menu.input.read(cx).text().trim()),
                    )),
            );
        }
        Some(
            div()
                .id("menu-layer")
                .absolute()
                .inset_0()
                .occlude()
                .when(wide, |layer| layer.bg(t.shade))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| this.close_menu(window, cx)),
                )
                .child(
                    div()
                        .id("menu")
                        .role(Role::Menu)
                        .absolute()
                        .left(left)
                        .top(top)
                        .w(px(width))
                        .when(max_height < f32::MAX, |box_| box_.max_h(px(max_height)))
                        .when(choice.is_some(), |box_| {
                            box_.track_focus(&menu.choice_focus)
                                .on_key_down(cx.listener(Self::on_choice_key))
                        })
                        .when(header.is_some(), |box_| {
                            box_.on_key_down(cx.listener(Self::on_menu_key))
                        })
                        .flex()
                        .flex_col()
                        .rounded(px(10.))
                        .bg(t.pop)
                        .shadow(t.menu_shadow())
                        .overflow_hidden()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .when_some(header, |box_, header| {
                            let raised = t.raised;
                            let swap = header.swap.clone();
                            let other = format!("⇥ {}", header.other);
                            box_.child(
                                small(12.)
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .h(px(MENU_HEADER_HEIGHT))
                                    .pl(px(12.))
                                    .pr(px(6.))
                                    .text_color(t.faint)
                                    .child(
                                        div()
                                            .text_color(t.muted)
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(header.title),
                                    )
                                    .child("·")
                                    .child(div().min_w_0().truncate().child(header.subject.clone()))
                                    .child(
                                        div()
                                            .id("menu-swap")
                                            .role(Role::Button)
                                            .aria_label(other.clone())
                                            .ml_auto()
                                            .flex_none()
                                            .px(px(6.))
                                            .rounded(px(5.))
                                            .hover(move |style| style.bg(raised))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                swap(this, window, cx);
                                            }))
                                            .child(other),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .flex_none()
                                .h(px(input_height))
                                .when(input_height > 0., |input| {
                                    input.border_b_1().border_color(t.line)
                                })
                                .overflow_hidden()
                                .px(px(if wide { 16. } else { 12. }))
                                .flex()
                                .items_center()
                                .text_color(t.ink)
                                .when(wide, |input| input.text_size(px(15.)))
                                .map(|input| match &choice {
                                    Some(title) => input
                                        .text_size(px(12.))
                                        .text_color(t.faint)
                                        .child(title.clone()),
                                    None => input.child(menu.input.clone()),
                                }),
                        )
                        .map(|box_| match calendar {
                            Some(calendar) => box_.child(Self::render_calendar(calendar, cx, &t)),
                            None => box_.child(rows),
                        })
                        .when_some(note, |box_, note| {
                            box_.child(
                                small(12.)
                                    .flex_none()
                                    .px(px(12.))
                                    .py(px(8.))
                                    .border_t_1()
                                    .border_color(t.line)
                                    .text_color(t.muted)
                                    .child(note),
                            )
                        })
                        .with_animation(
                            ("menu-enter", menu.run),
                            Animation::new(MENU_ENTER),
                            move |menu, delta| {
                                let eased = 1. - (1. - delta).powi(3);
                                menu.top(top - px(4. * (1. - eased))).opacity(eased)
                            },
                        ),
                ),
        )
    }

    pub(crate) fn render_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let toast = self.session.toast.as_ref()?;
        let t = theme(cx);
        let toast_button = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .flex_none()
                .text_color(t.accent)
                .font_weight(FontWeight::SEMIBOLD)
                .child(label)
        };
        let action = toast.action.as_ref();
        let restore = matches!(action, Some(ToastAction::RestoreDraft));
        Some(
            div()
                .absolute()
                .bottom(px(22.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("toast")
                        .role(Role::Alert)
                        .aria_label(toast.message.clone())
                        .max_w(relative(0.7))
                        .min_h(px(34.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(12.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(t.pop)
                        .shadow(t.menu_shadow())
                        .text_color(t.body)
                        .occlude()
                        .child(div().min_w_0().truncate().child(toast.message.clone()))
                        .when(toast.undo, |el| {
                            el.child(toast_button("toast-undo", "Undo".into()).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.session.run_toast_undo();
                                    cx.notify();
                                }),
                            ))
                        })
                        .when_some(action, |el, action| {
                            el.child(
                                toast_button("toast-action", action.label().into()).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        if restore {
                                            this.session.toast = None;
                                            this.restore_draft(window, cx);
                                        } else {
                                            this.session.run_toast_action();
                                        }
                                        cx.notify();
                                    }),
                                ),
                            )
                        }),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, size};

    use super::{Closeness, MENU_PADDING, closeness, px, revealed_offset, shortest_span};

    #[test]
    fn a_revealed_row_keeps_the_padding_around_it() {
        let view = Bounds::new(point(px(0.), px(100.)), size(px(200.), px(300.)));
        let row = |top: f32| Bounds::new(point(px(0.), px(top)), size(px(200.), px(30.)));

        let below = revealed_offset(row(500.), view, px(0.));
        assert_eq!(px(530.) + below, view.bottom() - px(MENU_PADDING));
        let above = revealed_offset(row(120.), view, px(-200.));
        assert_eq!(px(120.) + above, view.top() + px(MENU_PADDING));
        assert_eq!(revealed_offset(row(200.), view, px(-10.)), px(-10.));
    }

    /// The labels `query` leaves, as a menu lists them.
    fn ranked<'a>(query: &str, entries: &[(&'a str, &str)]) -> Vec<&'a str> {
        let mut found: Vec<(Closeness, &str)> = entries
            .iter()
            .filter_map(|(label, hint)| Some((closeness(query, label, hint)?, *label)))
            .collect();
        found.sort_by_key(|(closeness, _)| *closeness);
        found.into_iter().map(|(_, label)| label).collect()
    }

    fn labels<'a>(labels: &[&'a str]) -> Vec<(&'a str, &'static str)> {
        labels.iter().map(|label| (*label, "")).collect()
    }

    #[test]
    fn a_word_matches_across_the_gaps_in_a_name() {
        assert!(shortest_span("claudehist", "claude-history").is_some());
        assert!(shortest_span("ch", "claude-history").is_some());
        assert!(shortest_span("claudehistz", "claude-history").is_none());
        assert!(shortest_span("hc", "ch").is_none());
    }

    #[test]
    fn the_shortest_span_is_not_always_the_first_one() {
        assert_eq!(shortest_span("ab", "a---ab"), Some((2, 4)));
        assert_eq!(shortest_span("sa", "slack-agent"), Some((3, 0)));
    }

    #[test]
    fn tighter_matches_list_first() {
        let menu = labels(&[
            "s-l-a-c-k",
            "backslack",
            "team-slack",
            "slack-agent",
            "slack",
        ]);
        assert_eq!(
            ranked("slack", &menu),
            [
                "slack",
                "slack-agent",
                "team-slack",
                "backslack",
                "s-l-a-c-k"
            ],
        );
    }

    #[test]
    fn slack_agent_leads_for_each_way_of_typing_it() {
        let menu = labels(&[
            "a-slack-agent",
            "backslack",
            "skills-agent",
            "slack-agent",
            "snack-bag",
            "support-team-agent",
        ]);
        assert_eq!(ranked("slack", &menu)[0], "slack-agent");
        assert_eq!(ranked("slack-a", &menu)[0], "slack-agent");
        assert_eq!(ranked("slackag", &menu)[0], "slack-agent");
        assert_eq!(
            ranked("sa", &menu),
            [
                "slack-agent",
                "snack-bag",
                "a-slack-agent",
                "backslack",
                "skills-agent",
                "support-team-agent",
            ],
        );
    }

    #[test]
    fn a_compact_scatter_beats_a_spread_one() {
        let menu = labels(&["slack-big-agent", "slack-agent"]);
        assert_eq!(ranked("slackag", &menu), ["slack-agent", "slack-big-agent"]);
    }

    #[test]
    fn equal_matches_keep_their_order() {
        let menu = labels(&["slack-beta", "slack-agent", "slack-alpha"]);
        assert_eq!(
            ranked("slack", &menu),
            ["slack-beta", "slack-agent", "slack-alpha"],
        );
    }

    #[test]
    fn a_hint_counts_below_the_label_and_above_a_scatter() {
        let menu = [("sl-a-ck", ""), ("inbox", "slack"), ("team-slack", "")];
        assert_eq!(ranked("slack", &menu), ["team-slack", "inbox", "sl-a-ck"]);
    }

    #[test]
    fn a_hint_never_matches_loosely() {
        assert_eq!(closeness("slk", "inbox", "slack"), None);
    }

    #[test]
    fn an_equal_scatter_prefers_the_earlier_start() {
        let menu = labels(&["a-slack-agent", "slack-agent"]);
        assert_eq!(ranked("slackag", &menu), ["slack-agent", "a-slack-agent"]);
    }

    #[test]
    fn segments_end_at_any_separator() {
        let menu = labels(&["reagent", "slack_agent", "slack agent", "slack-agent"]);
        assert_eq!(
            ranked("agent", &menu),
            ["slack_agent", "slack agent", "slack-agent", "reagent"],
        );
    }

    #[test]
    fn the_whole_query_counts_before_its_words() {
        let menu = labels(&["agent slack", "slack agent tools", "slack agent"]);
        assert_eq!(
            ranked("slack agent", &menu),
            ["slack agent", "slack agent tools", "agent slack"],
        );
    }

    #[test]
    fn the_loosest_word_decides() {
        let menu = labels(&["sxlxaxcxk one", "one slack"]);
        assert_eq!(ranked("one slack", &menu), ["one slack", "sxlxaxcxk one"]);
    }

    #[test]
    fn every_word_must_match() {
        assert_eq!(closeness("slack zebra", "slack-agent", ""), None);
        assert_eq!(
            closeness("slack zebra", "slack-agent", "zebra"),
            Some(Closeness::Hint)
        );
    }
}

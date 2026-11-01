//! The window: navigation, the queue list, and the state that spans them.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use accesskit::Role;
use aven_core::choices::TaskStatus;
use aven_core::ids::TaskId;
use gpui::{
    Animation, AnimationExt, AnyElement, App, ClipboardItem, Context, Div, Entity, FocusHandle,
    Focusable, FontWeight, KeyContext, ListAlignment, ListOffset, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, Subscription, Window, div,
    linear_color_stop, linear_gradient, list, point, prelude::*, px, relative, svg,
};

use crate::actions::*;
use crate::adapter::{DateTone, Note, Prefs, Replies, Reply, SyncHealth};
use crate::bounce::{Bounce, ListBounce};
use crate::input::{Editor, EditorEvent, Mode};
use crate::kit::*;
use crate::measure::measured;
use crate::overlays::{Composer, Menu, TOAST_DURATION};
use crate::platform;
use crate::session::{Applied, PRIORITY_DURATION, SETTLE_DURATION, Session};
use crate::theme::{Theme, mix, theme};

const NAV_WIDTH_RANGE: (f32, f32) = (170., 340.);
const DETAIL_WIDTH_RANGE: (f32, f32) = (290., 620.);
/// The prototype's resize handles overlap each neighbouring panel by this much.
const SPLIT_OVERLAP: f32 = 1.;
/// The list keeps at least this width when panels are wide.
const LIST_MIN_WIDTH: f32 = 300.;
const SHEET_MARGIN: f32 = 6.;
/// At or below this list width, rows drop their labels and reasons.
const LIST_COLUMNS_MIN_WIDTH: f32 = 680.;
/// At or below this list width, rows also drop their refs and tags.
const LIST_REFS_MIN_WIDTH: f32 = 540.;
/// At or below this window width, the header drops its count and key hints.
const HEADER_HINTS_MIN_WINDOW_WIDTH: f32 = 1100.;
/// At or below this window width, the header shows only the view name.
const CRUMBS_MIN_WINDOW_WIDTH: f32 = 960.;
/// At or below this window width, the details panel starts closed.
const DETAIL_MIN_WINDOW_WIDTH: f32 = 1180.;
/// A section heading, whose 16.8px line GPUI cannot place exactly.
const SECTION_HEIGHT: f32 = 32.8;
const ROW_HEIGHT: f32 = 31.;
const GLIDE_DURATION: Duration = Duration::from_millis(140);
/// The farthest the highlight glides, enough for a neighbour across a section
/// header. A longer jump lands at once.
const GLIDE_REACH: f32 = 70.;
/// How long a row takes to reach its new place, and a group to fade.
const MOVE_DURATION: Duration = Duration::from_millis(200);

const NAV_VIEWS: [(&str, &str, &str); 10] = [
    ("queue", "Queue", "icons/ic-queue.svg"),
    ("ready", "Ready", "icons/ic-ready.svg"),
    ("inbox", "Inbox", "icons/ic-inbox.svg"),
    ("upcoming", "Upcoming", "icons/ic-upcoming.svg"),
    ("overdue", "Overdue", "icons/ic-overdue.svg"),
    ("blocked", "Blocked", "icons/ic-blocked.svg"),
    ("epics", "Epics", "icons/ic-epics.svg"),
    ("recurring", "Recurring Tasks", "icons/ic-recurring.svg"),
    ("done", "Done", "icons/ic-done.svg"),
    ("conflicts", "Conflicts", "icons/ic-conflicts.svg"),
];

/// GPUI snaps lengths to device pixels. Headings carry the remainder from one
/// to the next, so the rows below them do not drift down the list.
fn snapped_height(height: f32) -> f32 {
    (height * 2.).round() / 2.
}

#[derive(Clone, PartialEq)]
enum Item {
    Pad(f32),
    Section {
        index: usize,
        name: String,
        hint: String,
        count: usize,
        height: f32,
    },
    Row {
        task: usize,
        row: usize,
        id: TaskId,
    },
    Empty,
}

impl Item {
    fn height(&self) -> f32 {
        match self {
            Self::Pad(height) | Self::Section { height, .. } => *height,
            Self::Row { .. } => ROW_HEIGHT,
            Self::Empty => 0.,
        }
    }

    /// Whether both are the same list entry, so scroll can stay on it.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Section { name: a, .. }, Self::Section { name: b, .. }) => a == b,
            (Self::Row { id: a, .. }, Self::Row { id: b, .. }) => a == b,
            _ => self == other,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Split {
    Nav,
    Detail,
}

#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    run: usize,
    at: Instant,
}

impl Glide {
    fn live(self) -> bool {
        self.at.elapsed() < GLIDE_DURATION
    }

    fn top(self, delta: f32) -> f32 {
        self.from + (self.to - self.from) * ease_out(delta)
    }

    fn top_now(self) -> f32 {
        let delta = self.at.elapsed().as_secs_f32() / GLIDE_DURATION.as_secs_f32();
        self.top(delta.min(1.))
    }
}

fn ease_out(delta: f32) -> f32 {
    1. - (1. - delta).powi(3)
}

/// How far an entry that started `from` its place still is at `delta`.
fn offset_at(from: f32, delta: f32) -> f32 {
    from * (1. - ease_out(delta.min(1.)))
}

/// A section heading fading where its group used to be.
#[derive(Clone)]
struct LeftSection {
    name: String,
    hint: String,
    count: usize,
    height: f32,
    /// From the top of the list viewport.
    top: f32,
}

/// A list change made by a local write: rows glide from where they were, and
/// groups fade in or out. Live state is keyed on wall time, so a row scrolled
/// back into view does not replay it.
#[derive(Default)]
struct ListMotion {
    at: Option<Instant>,
    /// How far above or below its place each row starts, and its animation.
    rows: HashMap<TaskId, (f32, usize)>,
    /// The same for the headings of groups that stay.
    sections: HashMap<String, (f32, usize)>,
    entered: HashMap<String, usize>,
    left: Vec<(LeftSection, usize)>,
}

impl ListMotion {
    fn live(&self) -> bool {
        self.at.is_some_and(|at| at.elapsed() < MOVE_DURATION)
    }

    fn row(&self, id: &TaskId) -> Option<(f32, usize)> {
        self.rows.get(id).copied().filter(|_| self.live())
    }

    fn section(&self, name: &str) -> Option<(f32, usize)> {
        self.sections.get(name).copied().filter(|_| self.live())
    }

    fn entered(&self, name: &str) -> Option<usize> {
        self.entered.get(name).copied().filter(|_| self.live())
    }

    /// How far an entry still is from its place, so a new move carries on from it.
    fn remaining(&self, start: Option<(f32, usize)>) -> f32 {
        let (Some((from, _)), Some(at)) = (start, self.at) else {
            return 0.;
        };
        let delta = at.elapsed().as_secs_f32() / MOVE_DURATION.as_secs_f32();
        offset_at(from, delta)
    }

    /// What changed between two lists at one scroll position. A row that moves
    /// more than a screen lands at once.
    fn between(
        &self,
        old: &[Item],
        new: &[Item],
        scrolled: f32,
        viewport: f32,
        mut next_run: impl FnMut() -> usize,
    ) -> Self {
        let mut rows = HashMap::new();
        let mut sections = HashMap::new();
        let mut top = 0.;
        for item in old {
            match item {
                Item::Row { id, .. } => {
                    rows.insert(id, top);
                }
                Item::Section { name, .. } => {
                    sections.insert(name, (top, item));
                }
                _ => {}
            }
            top += item.height();
        }
        let mut next = Self {
            at: Some(Instant::now()),
            ..Self::default()
        };
        let mut top = 0.;
        for item in new {
            match item {
                Item::Row { id, .. } => {
                    if let Some(was) = rows.get(id) {
                        let from = was - top + self.remaining(self.row(id));
                        if from.abs() >= 0.5 && from.abs() <= viewport {
                            next.rows.insert(id.clone(), (from, next_run()));
                        }
                    }
                }
                Item::Section { name, .. } => match sections.remove(name) {
                    Some((was, _)) => {
                        let from = was - top + self.remaining(self.section(name));
                        if from.abs() >= 0.5 && from.abs() <= viewport {
                            next.sections.insert(name.clone(), (from, next_run()));
                        }
                    }
                    None => {
                        next.entered.insert(name.clone(), next_run());
                    }
                },
                _ => {}
            }
            top += item.height();
        }
        for (top, item) in sections.into_values() {
            if let Item::Section {
                name,
                hint,
                count,
                height,
                ..
            } = item
            {
                let left = LeftSection {
                    name: name.clone(),
                    hint: hint.clone(),
                    count: *count,
                    height: *height,
                    top: top - scrolled,
                };
                next.left.push((left, next_run()));
            }
        }
        next
    }
}

/// Paragraphs as the read-only editor shows them, without Markdown marks.
fn body_text(body: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut text = String::new();
    let mut bold = Vec::new();
    for source in body.split('\n').filter(|line| !line.is_empty()) {
        if !text.is_empty() {
            text.push('\n');
        }
        let source = match source.strip_prefix("- ") {
            Some(rest) => {
                text.push_str("•  ");
                rest
            }
            None => source,
        };
        for (ix, part) in source.split("**").enumerate() {
            let start = text.len();
            text.push_str(&part.replace('`', ""));
            if ix % 2 == 1 {
                bold.push(start..text.len());
            }
        }
    }
    (text, bold)
}

pub struct Root {
    /// Draws the active window look without key status, for offscreen scenes.
    pub always_active: bool,
    pub(crate) focus: FocusHandle,
    pub(crate) session: Session,
    items: Vec<Item>,
    list: ListState,
    bounce: Rc<RefCell<Bounce>>,
    glide: Option<Glide>,
    list_motion: ListMotion,
    pub(crate) details_open: bool,
    nav_open: bool,
    narrow: Option<bool>,
    /// Whether the list has room for its optional columns.
    list_wide: bool,
    /// Whether rows have room for refs and tags.
    list_refs: bool,
    pub(crate) prefs: Prefs,
    prefs_path: PathBuf,
    split_drag: Option<Split>,
    pub(crate) title: Entity<Editor>,
    /// The task the title editor holds, which may not be the selection yet.
    title_task: Option<TaskId>,
    /// The stored title the editor last showed. Only a difference from it is an
    /// edit, so a blur never writes back a title that changed elsewhere.
    title_base: String,
    /// The toast whose expiry is running.
    armed_toast: Option<usize>,
    pub(crate) description: Entity<Editor>,
    pub(crate) note: Entity<Editor>,
    pub(crate) note_views: Vec<(Note, Entity<Editor>)>,
    pub(crate) detail_scroll: ScrollHandle,
    pub(crate) menu: Option<Menu>,
    pub(crate) composer: Option<Composer>,
    pub(crate) composer_draft: String,
    pub(crate) anchors: Anchors,
    last_offset: Pixels,
    last_scroll: Option<Instant>,
    thumb_drag: Option<(Pixels, Pixels)>,
    subscriptions: Vec<Subscription>,
}

impl Root {
    pub fn new(
        session: Session,
        mut replies: Replies,
        prefs_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| Editor::new(Mode::Wrap, "Task title", window, cx));
        let description = cx.new(|cx| {
            Editor::new(Mode::Multi, "", window, cx)
                .read_only()
                .paragraph_gap(px(8.))
        });
        let note = cx.new(|cx| {
            Editor::new(
                Mode::Multi,
                "Add a note. Enter saves, Shift+Enter adds a line.",
                window,
                cx,
            )
            .max_height(px(206.))
        });
        let subscriptions = vec![
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.subscribe_in(&note, window, Self::on_note_event),
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];
        cx.spawn_in(window, async move |this, cx| {
            while let Some(reply) = replies.recv().await {
                let applied = this.update_in(cx, |this, window, cx| {
                    this.take_reply(reply, window, cx);
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
        let focus = cx.focus_handle();
        let prefs = Prefs::load(&prefs_path);
        let mut this = Self {
            always_active: false,
            focus,
            session,
            items: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(300.)).measure_all(),
            bounce: Rc::default(),
            glide: None,
            list_motion: ListMotion::default(),
            details_open: true,
            nav_open: true,
            narrow: None,
            list_wide: true,
            list_refs: true,
            prefs,
            prefs_path,
            split_drag: None,
            title,
            title_task: None,
            title_base: String::new(),
            armed_toast: None,
            description,
            note,
            note_views: Vec::new(),
            detail_scroll: ScrollHandle::new(),
            menu: None,
            composer: None,
            composer_draft: String::new(),
            anchors: Rc::default(),
            last_offset: px(0.),
            last_scroll: None,
            thumb_drag: None,
            subscriptions,
        };
        this.rebuild(false);
        this.load_selection(window, cx);
        window.focus(&this.focus, cx);
        this
    }

    /// Follows the system's appearance and reduced motion setting.
    pub fn follow_system(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_system(window, cx);
        self.subscriptions
            .push(cx.observe_window_appearance(window, |this, window, cx| {
                this.sync_system(window, cx);
            }));
        self.subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                this.sync_system(window, cx);
            }));
    }

    fn sync_system(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reduced = platform::reduce_motion();
        cx.set_reduce_motion(reduced);
        self.session.motion.set_reduced(reduced);
        if self.active(window) || !cx.has_global::<Theme>() {
            let dark = matches!(
                window.appearance(),
                gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
            );
            if !cx.has_global::<Theme>() || theme(cx).dark != dark {
                cx.set_global(if dark { Theme::dark() } else { Theme::light() });
            }
        }
        cx.notify();
    }

    /// The list's scroll extent, for layout measurement.
    pub fn scroll_max(&self) -> f32 {
        f32::from(self.list.max_offset_for_scrollbar().y)
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub(crate) fn composer_open(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| !composer.closing)
    }

    fn active(&self, window: &Window) -> bool {
        self.always_active || window.is_window_active()
    }

    fn build_items(&self) -> Vec<Item> {
        let snapshot = &self.session.snapshot;
        let mut items = vec![Item::Pad(4.)];
        let mut row = 0;
        let mut carried = 0.;
        for (index, group) in snapshot.groups.iter().enumerate() {
            let height = snapped_height(SECTION_HEIGHT + carried);
            carried += SECTION_HEIGHT - height;
            items.push(Item::Section {
                index,
                name: group.name.clone(),
                hint: group.hint.clone(),
                count: group.rows.len(),
                height,
            });
            for task in &group.rows {
                let id = snapshot.tasks[*task].id.clone();
                items.push(Item::Row {
                    task: *task,
                    row,
                    id,
                });
                row += 1;
            }
        }
        if items.len() == 1 {
            items.push(Item::Empty);
        }
        items.push(Item::Pad(40.));
        items
    }

    /// Rebuilds list entries, replacing only those that moved. Entries around
    /// them keep their measured heights and the scroll position stays on its
    /// entry. After a local write it stays at its offset instead, because the
    /// entry may be the row that left for another group.
    fn rebuild(&mut self, moved: bool) {
        let items = self.build_items();
        let top = self.list.logical_scroll_top();
        let shared = items.len().min(self.items.len());
        let start = (0..shared)
            .find(|ix| !items[*ix].same(&self.items[*ix]))
            .unwrap_or(shared);
        let end = (0..shared - start)
            .find(|ix| !items[items.len() - 1 - ix].same(&self.items[self.items.len() - 1 - ix]))
            .unwrap_or(shared - start);
        let (old_end, new_end) = (self.items.len() - end, items.len() - end);
        if start < old_end || start < new_end {
            self.list.splice(start..old_end, new_end - start);
            // Measures the new entries; `measure_all` otherwise only runs once.
            self.list.remeasure_items(start..start);
            self.glide = None;
            if moved {
                let scrolled = f32::from(top.offset_in_item)
                    + self.items[..top.item_ix.min(self.items.len())]
                        .iter()
                        .map(Item::height)
                        .sum::<f32>();
                if !self.session.motion.reduced() {
                    let viewport = f32::from(self.list.viewport_bounds().size.height);
                    let motion = &mut self.session.motion;
                    self.list_motion =
                        self.list_motion
                            .between(&self.items, &items, scrolled, viewport, || {
                                motion.next_run()
                            });
                }
                self.list.scroll_to(list_offset_at(&items, scrolled));
            } else {
                // A splice over the scroll anchor drops it to the splice's start.
                let anchor = (start..old_end)
                    .contains(&top.item_ix)
                    .then(|| &self.items[top.item_ix]);
                if let Some(ix) = anchor.and_then(|old| items.iter().position(|new| new.same(old)))
                {
                    self.list.scroll_to(ListOffset { item_ix: ix, ..top });
                }
            }
        }
        self.items = items;
    }

    /// Shows the session's list as it now stands.
    pub fn reload_list(&mut self, cx: &mut Context<Self>) {
        self.rebuild(false);
        cx.notify();
    }

    /// Takes one reply from the store.
    pub fn take_reply(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        let applied = self.session.apply(reply);
        self.take_applied(applied, window, cx);
    }

    pub(crate) fn take_applied(
        &mut self,
        applied: Applied,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if applied.list {
            self.rebuild(applied.moved.is_some());
        }
        if let Some(title) = applied.composer_draft
            && self.composer_draft.is_empty()
        {
            self.composer_draft = title;
        }
        if applied.draft
            && let Some(id) = self.session.selected.clone()
        {
            let typed = self.note.read(cx).text().to_string();
            self.session.merge_note_draft(&id, &typed);
        }
        if applied.selection {
            self.commit_title(cx);
            self.load_selection(window, cx);
            self.reveal_selection();
        } else {
            if applied.moved.is_some() && applied.moved == self.session.selected {
                self.reveal_selection();
            }
            if applied.draft {
                self.load_note_draft(cx);
            }
            self.sync_editors(window, cx);
        }
        self.arm_toast(cx);
        cx.notify();
    }

    fn arm_toast(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.session.toast.as_ref().map(|toast| toast.run) else {
            return;
        };
        if self.armed_toast.replace(run) == Some(run) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            this.update(cx, |this, cx| {
                if this
                    .session
                    .toast
                    .as_ref()
                    .is_some_and(|toast| toast.run == run)
                {
                    this.session.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Brings open editors up to date with stored data. Focused editors keep
    /// their text, with any IME composition inside it.
    fn sync_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self
            .session
            .selected
            .as_ref()
            .and_then(|id| self.session.snapshot.row(id));
        let title = row.map(|row| row.title.clone());
        if let Some(title) = title
            && !self.title.read(cx).focus.is_focused(window)
        {
            self.show_title(title, cx);
        }
        let Some(detail) = self.session.detail.clone() else {
            return;
        };
        let (text, bold) = body_text(&detail.description);
        if self.description.read(cx).text() != text {
            self.description.update(cx, |editor, cx| {
                editor.set_text(&text, cx);
                editor.set_bold(bold);
            });
        }
        let shown = self.note_views.iter().map(|(note, _)| note);
        if !shown.eq(detail.notes.iter()) {
            self.note_views = detail
                .notes
                .iter()
                .map(|note| (note.clone(), self.note_view(&note.body, window, cx)))
                .collect();
        }
    }

    fn show_title(&mut self, title: String, cx: &mut Context<Self>) {
        if self.title.read(cx).text() != title {
            self.title
                .update(cx, |editor, cx| editor.set_text(&title, cx));
        }
        self.title_base = title;
    }

    fn note_view(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Editor> {
        let (text, bold) = body_text(text);
        cx.new(|cx| {
            let mut editor = Editor::new(Mode::Multi, "", window, cx)
                .read_only()
                .paragraph_gap(px(8.));
            editor.set_text(&text, cx);
            editor.set_bold(bold);
            editor
        })
    }

    fn load_note_draft(&mut self, cx: &mut Context<Self>) {
        let draft = self
            .session
            .selected
            .as_ref()
            .and_then(|id| self.session.note_drafts.get(id))
            .cloned()
            .unwrap_or_default();
        self.note.update(cx, |note, cx| note.set_text(&draft, cx));
    }

    /// Points the details editors at the current selection.
    pub(crate) fn load_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self
            .session
            .selected
            .as_ref()
            .and_then(|id| self.session.snapshot.row(id));
        self.title_task = row.map(|row| row.id.clone());
        let title = row.map(|row| row.title.clone()).unwrap_or_default();
        self.show_title(title, cx);
        self.load_note_draft(cx);
        self.note_views.clear();
        self.description.update(cx, |editor, cx| {
            editor.set_text("", cx);
            editor.set_bold(Vec::new());
        });
        self.detail_scroll.set_offset(point(px(0.), px(0.)));
        self.sync_editors(window, cx);
    }

    fn commit_title(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.title_task.clone() else {
            return;
        };
        let text = self.title.read(cx).text().trim().to_string();
        if text == self.title_base {
            return;
        }
        if text.is_empty() {
            let saved = self.session.snapshot.row(&id).map(|row| row.title.clone());
            self.show_title(saved.unwrap_or_default(), cx);
        } else {
            self.title_base = text.clone();
            self.session.set_title(&id, text);
        }
    }

    fn keep_note_draft(&mut self, cx: &App) {
        let Some(id) = self.session.selected.clone() else {
            return;
        };
        let draft = self.note.read(cx).text().to_string();
        if draft.is_empty() {
            self.session.note_drafts.remove(&id);
        } else {
            self.session.note_drafts.insert(id, draft);
        }
    }

    pub(crate) fn select(&mut self, id: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.selected.as_ref() == Some(&id) {
            return;
        }
        self.commit_title(cx);
        self.keep_note_draft(cx);
        self.start_glide(&id);
        self.session.select(id);
        self.load_selection(window, cx);
        self.reveal_selection();
        cx.notify();
    }

    fn item_of(&self, id: &TaskId) -> Option<usize> {
        self.items
            .iter()
            .position(|item| matches!(item, Item::Row { id: row, .. } if row == id))
    }

    pub(crate) fn selected_row_bounds(&self) -> gpui::Bounds<Pixels> {
        let item = self
            .session
            .selected
            .as_ref()
            .and_then(|id| self.item_of(id));
        item.and_then(|item| self.list.bounds_for_item(item))
            .unwrap_or_default()
    }

    fn reveal_selection(&self) {
        if let Some(position) = self
            .session
            .selected
            .as_ref()
            .and_then(|id| self.item_of(id))
        {
            self.list.scroll_to_reveal_item(position);
        }
    }

    /// Where a task's row sits in the list content, while it is laid out.
    fn row_top(&self, id: &TaskId) -> Option<f32> {
        let bounds = self.list.bounds_for_item(self.item_of(id)?)?;
        let viewport = self.list.viewport_bounds();
        Some(f32::from(bounds.top() - viewport.top()) + self.list_offset())
    }

    fn list_offset(&self) -> f32 {
        -f32::from(self.list.scroll_px_offset_for_scrollbar().y)
    }

    /// Moves the highlight from the selected row, or from wherever it has
    /// got to, so holding a key keeps it travelling.
    fn start_glide(&mut self, id: &TaskId) {
        if self.session.motion.reduced() {
            self.glide = None;
            return;
        }
        let viewport = f32::from(self.list.viewport_bounds().size.height);
        let offset = self.list_offset();
        let from = self
            .glide
            .filter(|glide| glide.live())
            .map(Glide::top_now)
            .or_else(|| {
                self.session
                    .selected
                    .as_ref()
                    .and_then(|sel| self.row_top(sel))
            })
            .filter(|top| (offset - ROW_HEIGHT..offset + viewport).contains(top));
        let target = from.zip(self.row_top(id));
        let near = target.filter(|(from, to)| (from - to).abs() <= GLIDE_REACH);
        self.glide = near.map(|(from, to)| Glide {
            from,
            to,
            run: self.session.motion.next_run(),
            at: Instant::now(),
        });
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.session.neighbour(delta) {
            self.select(id, window, cx);
        }
    }

    fn move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    fn move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, window, cx);
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _: &mut Window, cx: &mut Context<Self>) {
        cx.set_global(if theme(cx).dark {
            Theme::light()
        } else {
            Theme::dark()
        });
        cx.notify();
    }

    fn toggle_details(&mut self, _: &ToggleDetails, _: &mut Window, cx: &mut Context<Self>) {
        self.details_open = !self.details_open;
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.nav_open = !self.nav_open;
        cx.notify();
    }

    pub(crate) fn mark_done(&mut self, _: &MarkDone, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.session.selected.clone() {
            self.session.toggle_status(&id, TaskStatus::Done);
            self.after_status_motion(cx);
        }
    }

    fn cancel_task(&mut self, _: &CancelTask, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.session.selected.clone() {
            self.session.toggle_status(&id, TaskStatus::Canceled);
            cx.notify();
        }
    }

    fn refresh_view(&mut self, cx: &mut Context<Self>) {
        self.session.refresh_view();
        cx.notify();
    }

    pub(crate) fn after_status_motion(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        // The row returns to its resting elements when the motion ends.
        self.repaint_after(SETTLE_DURATION, cx);
    }

    pub(crate) fn repaint_after(&self, delay: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    fn edit_title(&mut self, _: &EditTitle, window: &mut Window, cx: &mut Context<Self>) {
        self.details_open = true;
        let focus = self.title.read(cx).focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn copy_ref(&mut self, _: &CopyRef, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .session
            .selected
            .as_ref()
            .and_then(|id| self.session.snapshot.row(id))
        else {
            return;
        };
        let (id, reference) = (row.id.clone(), row.reference.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(reference));
        self.session.motion.copied(&id);
        self.repaint_after(crate::session::COPIED_DURATION, cx);
        cx.notify();
    }

    fn on_title_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Submit => window.focus(&self.focus, cx),
            EditorEvent::Cancel => {
                let saved = self
                    .title_task
                    .as_ref()
                    .and_then(|id| self.session.snapshot.row(id))
                    .map(|row| row.title.clone());
                self.show_title(saved.unwrap_or_default(), cx);
                window.focus(&self.focus, cx);
            }
            EditorEvent::Blur => {
                self.commit_title(cx);
                self.sync_editors(window, cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_note_event(
        &mut self,
        note: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Submit => {
                let text = note.read(cx).text().trim().to_string();
                let Some(id) = self.session.selected.clone().filter(|_| !text.is_empty()) else {
                    return;
                };
                self.session.add_note(&id, text);
                note.update(cx, |note, cx| note.set_text("", cx));
                cx.notify();
            }
            EditorEvent::Changed => self.keep_note_draft(cx),
            EditorEvent::Cancel => window.focus(&self.focus, cx),
            _ => {}
        }
    }

    fn close_window(&mut self, _: &CloseWindow, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    fn render_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        match self.items[ix].clone() {
            Item::Pad(height) => div().h(px(height)).into_any_element(),
            Item::Empty => div()
                .w_full()
                .pt(px(48.))
                .flex()
                .justify_center()
                .text_size(px(15.))
                .child("No tasks here")
                .into_any_element(),
            Item::Section {
                index,
                name,
                hint,
                count,
                height,
            } => {
                let entered = self.list_motion.entered(&name);
                let moved = self.list_motion.section(&name);
                let section = self.section_heading(
                    measured(div(), format!("section-{index}")),
                    name,
                    hint,
                    count,
                    height,
                    &t,
                );
                let section = div().px(px(8.)).child(section);
                match (entered, moved) {
                    (Some(run), _) => section
                        .with_animation(
                            ("section-enter", run),
                            Animation::new(MOVE_DURATION),
                            |section, delta| section.opacity(delta),
                        )
                        .into_any_element(),
                    (None, Some(start)) => glide_to_place(section, start),
                    (None, None) => section.into_any_element(),
                }
            }
            Item::Row { task, row, .. } => self.render_row(ix, task, row, window, cx),
        }
    }

    fn section_heading(
        &self,
        base: Div,
        name: String,
        hint: String,
        count: usize,
        height: f32,
        t: &Theme,
    ) -> Div {
        base.h(px(height))
            .px(px(8.))
            .pt(px(12.))
            .pb(px(4.))
            .flex()
            .items_baseline()
            .gap(px(8.))
            .text_size(px(12.))
            .line_height(px(16.8))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.muted)
            .child(name)
            .child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(t.faint)
                    .child(count.to_string()),
            )
            .when(self.list_wide, |section| {
                section.child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(t.faint)
                        .text_size(px(11.))
                        .child(hint),
                )
            })
    }

    /// The status mark, popping while the task has just been completed.
    pub(crate) fn task_glyph(&self, id: &TaskId, status: TaskStatus, t: &Theme) -> AnyElement {
        match self.session.motion.completion(id) {
            Some((previous, run)) => pop_glyph(previous, run, t).into_any_element(),
            None => status_glyph(status, t).into_any_element(),
        }
    }

    /// The priority icon, crossfading while it has just changed.
    pub(crate) fn task_priority(
        &self,
        id: &TaskId,
        priority: aven_core::choices::TaskPriority,
        ids: (&'static str, &'static str),
        t: &Theme,
    ) -> AnyElement {
        let Some((previous, run)) = self.session.motion.priority_change(id) else {
            return priority_icon(priority, t).into_any_element();
        };
        let layer = |priority| div().absolute().child(priority_icon(priority, t));
        let animation = Animation::new(PRIORITY_DURATION);
        div()
            .relative()
            .flex_none()
            .size(px(16.))
            .child(
                layer(previous).with_animation((ids.0, run), animation.clone(), |old, delta| {
                    old.opacity(1. - delta)
                }),
            )
            .child(
                layer(priority)
                    .with_animation((ids.1, run), animation, |new, delta| new.opacity(delta)),
            )
            .into_any_element()
    }

    /// The row's title, dimming while the task has just been completed.
    fn task_title(&self, id: &TaskId, title: String, status: TaskStatus, t: &Theme) -> AnyElement {
        let closed = matches!(status, TaskStatus::Done | TaskStatus::Canceled);
        let (ink, faint) = (t.ink, t.faint);
        let label = div()
            .min_w_0()
            .overflow_hidden()
            .text_color(if closed { faint } else { ink })
            .child(div().truncate().child(title));
        let run = self.session.motion.completion(id).map(|(_, run)| run);
        let body = match (status, run) {
            (TaskStatus::Done, Some(run)) => label
                .with_animation(
                    ("title-dim", run),
                    Animation::new(SETTLE_DURATION),
                    move |title, delta| {
                        title.text_color(mix(ink, faint, ((delta - 0.3) / 0.4).clamp(0., 1.)))
                    },
                )
                .into_any_element(),
            _ => label.into_any_element(),
        };
        div()
            .flex_1()
            .min_w(px(120.))
            .flex()
            .child(body)
            .into_any_element()
    }

    fn render_row(
        &mut self,
        ix: usize,
        task: usize,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let data = self.session.snapshot.tasks[task].clone();
        let id = data.id.clone();
        let selected = self.session.selected.as_ref() == Some(&id);
        let below_selected = matches!(
            &self.items[ix - 1],
            Item::Row { id, .. } if self.session.selected.as_ref() == Some(id)
        );
        let active = self.active(window);
        let gliding = self.glide.is_some_and(Glide::live);
        let wide = self.list_wide;
        let date_color = match data.date_tone {
            DateTone::Soon => t.warn,
            DateTone::Late => t.bad,
            DateTone::Plain => t.faint,
        };
        let labels = data
            .labels
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let (select_id, toggle_id) = (id.clone(), id.clone());
        let body = measured(div(), format!("row-{row}"))
            .relative()
            .id(("row", row))
            .role(Role::ListItem)
            .aria_label(format!("{} {}", data.reference, data.title))
            .aria_selected(selected)
            .h(px(ROW_HEIGHT))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(9.))
            .rounded(px(6.))
            .when(selected && !gliding, |row| {
                let (bg, ring) = t.selection(active);
                row.bg(bg).shadow(vec![t.ring(ring)])
            })
            .when(!selected && !below_selected, |row| {
                row.hover(|style| style.bg(t.panel))
            })
            // The selection's ring reaches 1px into this row and stays visible.
            .when(below_selected, |row| {
                row.group("below-selected").child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .rounded(px(6.))
                        .group_hover("below-selected", |style| style.bg(t.panel)),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.select(select_id.clone(), window, cx);
                }),
            )
            .child(div().flex_none().w(px(3.)).h(px(16.)).ml(px(-5.)))
            .child(self.task_priority(&id, data.priority, ("pri-row-old", "pri-row-new"), &t))
            .child(
                div()
                    .id(("glyph", row))
                    .role(Role::CheckBox)
                    .aria_label("Toggle done")
                    .flex_none()
                    .size(px(14.))
                    .rounded_full()
                    .hover(|style| style.shadow(vec![spread(t.raised, 3.)]))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.session.toggle_status(&toggle_id, TaskStatus::Done);
                        this.after_status_motion(cx);
                    }))
                    .child(self.task_glyph(&id, data.status, &t)),
            )
            .when(self.list_refs, |row| {
                row.child(ref_label(&data.reference, &data.project, Some(74.), &t))
            })
            .child(self.task_title(&id, data.title, data.status, &t))
            .when(data.recurring, |row| {
                row.child(small(12.).flex_none().text_color(t.faint).child("↻"))
            })
            .when_some(data.reason.filter(|_| wide), |row, (reason, bad)| {
                row.child(
                    small(12.)
                        .flex_none()
                        .text_color(if bad { t.bad } else { t.warn })
                        .child(reason),
                )
            })
            .when(data.blocked && self.list_refs, |row| {
                row.child(small(12.).flex_none().text_color(t.warn).child("Blocked"))
            })
            .when_some(data.progress, |row, progress| {
                row.child(
                    div()
                        .flex_none()
                        .w(px(52.))
                        .font_family(MONO)
                        .text_size(px(12.))
                        .text_color(t.faint)
                        .child(progress),
                )
            })
            .when(wide, |row| {
                row.child(
                    small(12.)
                        .flex_none()
                        .w(px(112.))
                        .text_right()
                        .truncate()
                        .text_color(t.faint)
                        .child(labels),
                )
            })
            .child(
                small(12.)
                    .flex_none()
                    .w(px(64.))
                    .text_right()
                    .text_color(date_color)
                    .child(data.due),
            );
        let place = div().px(px(8.)).child(body);
        match self.list_motion.row(&id) {
            Some(start) => glide_to_place(place, start),
            None => place.into_any_element(),
        }
    }

    fn render_nav(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let active = self.active(window);
        let snapshot = &self.session.snapshot;
        let header = |label: &'static str| {
            div()
                .px(px(8.))
                .pt(px(14.))
                .pb(px(4.))
                .text_size(px(11.))
                .line_height(px(15.4))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.faint)
                .child(label)
        };
        let mut body = div()
            .id("nav-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(8.))
            .pb(px(8.))
            .child(header("Workspaces"));
        for (ix, workspace) in snapshot.workspaces.iter().enumerate() {
            let on = workspace.name == snapshot.workspace;
            body = body.child(
                nav_item(("workspace", ix), &t)
                    .when(on, |item| {
                        item.text_color(t.ink).font_weight(FontWeight::MEDIUM)
                    })
                    .child(workspace.name.clone())
                    .child(nav_count(workspace.count.to_string(), false, &t)),
            );
        }
        body = body.child(header("Views"));
        for (ix, (key, name, icon_path)) in NAV_VIEWS.into_iter().enumerate() {
            let on = key == "queue";
            let count = snapshot
                .views
                .iter()
                .find(|(view, _)| *view == key)
                .map_or(0, |(_, count)| *count);
            let hot = matches!(key, "overdue" | "conflicts") && count > 0;
            body = body.child(
                nav_item(("view", ix), &t)
                    .when(on, |item| {
                        item.bg(if active { t.tint } else { t.raised })
                            .text_color(t.ink)
                            .font_weight(FontWeight::MEDIUM)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_view(cx)))
                    })
                    .child(icon(icon_path, if on { t.accent } else { t.faint }))
                    .child(name)
                    .child(nav_count(
                        if count == 0 {
                            String::new()
                        } else {
                            count.to_string()
                        },
                        hot,
                        &t,
                    )),
            );
        }
        body = body
            .child(nav_item("more", &t).text_color(t.faint).child("More views"))
            .child(header("Projects"))
            .child(
                nav_item("all-projects", &t)
                    .text_color(t.ink)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon("icons/ic-projects.svg", t.faint))
                    .child("All projects"),
            );
        for (ix, project) in snapshot.projects.iter().enumerate() {
            body = body.child(
                nav_item(("project", ix), &t)
                    .child(
                        div()
                            .flex_none()
                            .mx(px(4.))
                            .size(px(8.))
                            .rounded(px(3.))
                            .bg(t.project_dot(&project.prefix)),
                    )
                    .child(project.name.clone())
                    .child(nav_count(project.count.to_string(), false, &t)),
            );
        }
        let (sync_color, sync_label) = match &self.session.sync {
            Some(SyncHealth::Synced) => (t.good, "Synced".to_string()),
            Some(SyncHealth::Waiting) => (t.warn, "Sync waiting".to_string()),
            Some(SyncHealth::Failed(error)) => (t.bad, format!("Sync failed: {error}")),
            Some(SyncHealth::VaultDeleted) => (t.bad, "Sync vault deleted".to_string()),
            Some(SyncHealth::NotSetUp) | None => (t.faint, "Sync is not set up".to_string()),
        };
        measured(div(), "nav")
            .flex_none()
            .w(px(self.prefs.nav_width))
            .h_full()
            .flex()
            .flex_col()
            .child(self.render_lights(cx))
            .child(body)
            .child(
                div()
                    .flex_none()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .text_size(px(12.))
                    .line_height(px(16.8))
                    .child(
                        nav_item("settings", &t)
                            .w_auto()
                            .h(px(28.))
                            .mb_0()
                            .text_color(t.muted)
                            .child(icon("icons/ic-settings.svg", t.faint))
                            .child("Settings"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("sync-state")
                            .role(Role::Image)
                            .aria_label(sync_label)
                            .px(px(6.))
                            .child(icon("icons/cloud.svg", sync_color)),
                    ),
            )
    }

    fn render_lights(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        div()
            .flex_none()
            .mt(px(6.))
            .h(px(43.))
            .w(if self.nav_open {
                relative(1.).into()
            } else {
                gpui::Length::from(px(112.))
            })
            .pl(px(80.))
            .flex()
            .items_center()
            .on_mouse_down(MouseButton::Left, drag_window)
            .child(
                div()
                    .id("nav-toggle")
                    .role(Role::Button)
                    .aria_label("Toggle sidebar")
                    .size(px(26.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|style| style.bg(t.raised))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_sidebar(&ToggleSidebar, window, cx)
                    }))
                    .child(
                        svg()
                            .path("icons/nav-toggle.svg")
                            .size(px(16.))
                            .text_color(t.muted),
                    ),
            )
    }

    fn render_top(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let active = self.active(window);
        let count = self.session.snapshot.ordered().count();
        let width = window.viewport_size().width;
        let hints = width > px(HEADER_HINTS_MIN_WINDOW_WIDTH);
        let crumbs = width > px(CRUMBS_MIN_WINDOW_WIDTH);
        measured(div(), "main-header")
            .flex_none()
            .h(px(44.))
            .pl(px(if self.nav_open { 16. } else { 0. }))
            .pr(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .on_mouse_down(MouseButton::Left, drag_window)
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(1.))
                    .bg(t.line),
            )
            .when(!self.nav_open, |top| top.child(self.render_lights(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_none()
                    .when(crumbs, |crumb| {
                        crumb
                            .child(
                                div()
                                    .text_color(t.muted)
                                    .child(self.session.snapshot.workspace.clone()),
                            )
                            .child(div().text_color(t.faint).child("›"))
                            .child(div().text_color(t.muted).child("All projects"))
                            .child(div().text_color(t.faint).child("›"))
                    })
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child("Queue"),
                    ),
            )
            .when(hints, |top| {
                top.child(
                    div()
                        .flex_none()
                        .text_color(t.faint)
                        .child(format!("{count} tasks")),
                )
            })
            .child(
                small(12.)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.faint)
                    .when(hints, |hint| {
                        hint.child("Open and available, most urgent first")
                    }),
            )
            .child(btn("review", "Review tasks", &t).child("Review tasks"))
            .child(btn("filter", "Filter", &t).child("Filter"))
            .child(
                btn("details", "Toggle details", &t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_details(&ToggleDetails, window, cx)
                    }))
                    .child(icon("icons/ic-details.svg", t.body))
                    .child("Details"),
            )
            .child(
                btn("search", "Search", &t)
                    .shadow(vec![t.inset_ring(t.line)])
                    .child("Search")
                    .when(hints, |search| {
                        search.child(small(12.).text_color(t.faint).child("/"))
                    }),
            )
            .child(
                btn("palette", "Commands", &t)
                    .shadow(vec![t.inset_ring(t.line)])
                    .child("⌘K"),
            )
            .child(
                bare_btn("new-task", "New task", &t)
                    .map(|button| {
                        if active {
                            button
                                .bg(linear_gradient(
                                    180.,
                                    linear_color_stop(t.fill_top, 0.),
                                    linear_color_stop(t.fill, 1.),
                                ))
                                .text_color(t.on_fill)
                                .hover(|style| style.bg(t.fill_h))
                        } else {
                            button
                                .bg(t.raised)
                                .text_color(t.muted)
                                .hover(|style| style.bg(t.panel))
                        }
                    })
                    .font_weight(FontWeight::MEDIUM)
                    .on_click(cx.listener(|this, _, window, cx| this.open_composer(window, cx)))
                    .child("New task")
                    .when(hints, |button| {
                        button.child(
                            small(12.)
                                .opacity(0.7)
                                .font_weight(FontWeight::NORMAL)
                                .child("a"),
                        )
                    }),
            )
    }

    fn render_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = theme(cx);
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        let max = self.list.max_offset_for_scrollbar().y;
        let viewport = self.list.viewport_bounds();
        let opacity = self.thumb_opacity(window);
        let track = viewport.size.height - px(8.);
        let thumb = (max > px(0.) && opacity > 0.).then(|| {
            let total = max + viewport.size.height;
            let height = (track * (viewport.size.height / total)).max(px(32.));
            let top = (track - height) * (offset / max).clamp(0., 1.);
            (top, height)
        });
        let (glide_bg, glide_ring) = t.selection(self.active(window));
        let glide = self.glide.filter(|glide| glide.live()).map(|glide| {
            let scrolled = f32::from(offset);
            div().absolute().inset_0().overflow_hidden().child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .h(px(ROW_HEIGHT))
                    .rounded(px(6.))
                    .bg(glide_bg)
                    .shadow(vec![t.ring(glide_ring)])
                    .with_animation(
                        ("selection-glide", glide.run),
                        Animation::new(GLIDE_DURATION),
                        move |bar, delta| bar.top(px(glide.top(delta) - scrolled)),
                    ),
            )
        });
        let left = if self.list_motion.live() {
            self.list_motion.left.clone()
        } else {
            Vec::new()
        };
        let left = left.into_iter().map(|(section, run)| {
            let top = section.top;
            let heading = self.section_heading(
                div(),
                section.name,
                section.hint,
                section.count,
                section.height,
                &t,
            );
            div().absolute().inset_0().overflow_hidden().child(
                div()
                    .absolute()
                    .top(px(top))
                    .left(px(8.))
                    .right(px(8.))
                    .child(heading)
                    .with_animation(
                        ("section-leave", run),
                        Animation::new(MOVE_DURATION),
                        |section, delta| section.opacity(1. - delta),
                    ),
            )
        });
        measured(div(), "list")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .children(glide)
            .children(left)
            .child(ListBounce::new(
                "list-bounce",
                self.bounce.clone(),
                self.list.clone(),
                list(self.list.clone(), cx.processor(Self::render_item)).size_full(),
            ))
            .when_some(thumb, |el, (top, height)| {
                el.child(
                    div()
                        .id("thumb")
                        .absolute()
                        .right(px(2.))
                        .top(px(4.) + top)
                        .w(px(7.))
                        .h(height)
                        .rounded_full()
                        .bg(t.bar)
                        .opacity(opacity)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.thumb_drag = Some((event.position.y, offset));
                                this.list.scrollbar_drag_started();
                            }),
                        ),
                )
            })
    }

    fn thumb_opacity(&mut self, window: &Window) -> f32 {
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        if offset != self.last_offset || self.thumb_drag.is_some() {
            self.last_offset = offset;
            self.last_scroll = Some(Instant::now());
        }
        let Some(at) = self.last_scroll else {
            return 0.;
        };
        let elapsed = at.elapsed().as_secs_f32();
        if elapsed > 1.05 {
            self.last_scroll = None;
            return 0.;
        }
        window.request_animation_frame();
        (1. - (elapsed - 0.8).max(0.) / 0.25).clamp(0., 1.)
    }

    fn split_handle(&self, split: Split, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .id(match split {
                Split::Nav => "split-nav",
                Split::Detail => "split-detail",
            })
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(7.))
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.split_drag = Some(split);
                }),
            )
    }

    fn drag_split(&mut self, split: Split, x: f32, window: &Window, cx: &mut Context<Self>) {
        let width = f32::from(window.viewport_size().width);
        match split {
            Split::Nav => {
                self.prefs.nav_width = x.clamp(NAV_WIDTH_RANGE.0, NAV_WIDTH_RANGE.1);
            }
            Split::Detail => {
                let wide = width - SHEET_MARGIN - x;
                self.prefs.detail_width = wide.clamp(DETAIL_WIDTH_RANGE.0, DETAIL_WIDTH_RANGE.1);
            }
        }
        cx.notify();
    }

    /// The details width that still leaves the list usable in this window.
    fn detail_width(&self, window: &Window) -> f32 {
        let nav = if self.nav_open {
            self.prefs.nav_width
        } else {
            0.
        };
        let room = f32::from(window.viewport_size().width) - nav - LIST_MIN_WIDTH;
        self.prefs.detail_width.min(room).max(DETAIL_WIDTH_RANGE.0)
    }
}

/// Moves an entry from its old place to where the list lays it out.
fn glide_to_place(entry: Div, (from, run): (f32, usize)) -> AnyElement {
    entry
        .relative()
        .with_animation(
            ("list-move", run),
            Animation::new(MOVE_DURATION),
            move |entry, delta| entry.top(px(offset_at(from, delta))),
        )
        .into_any_element()
}

/// The list position `scrolled` pixels down `items`.
fn list_offset_at(items: &[Item], scrolled: f32) -> ListOffset {
    let mut top = 0.;
    for (item_ix, item) in items.iter().enumerate() {
        if scrolled < top + item.height() {
            return ListOffset {
                item_ix,
                offset_in_item: px(scrolled - top),
            };
        }
        top += item.height();
    }
    ListOffset::default()
}

impl Focusable for Root {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let narrow = window.viewport_size().width <= px(DETAIL_MIN_WINDOW_WIDTH);
        if self.narrow != Some(narrow) {
            self.narrow = Some(narrow);
            self.details_open = !narrow;
        }
        let mut key_context = KeyContext::default();
        key_context.add("Aven");
        if self.menu.is_some() || self.composer_open() {
            key_context.add("Overlay");
        }
        let nav = self.nav_open.then(|| self.render_nav(window, cx));
        let top = self.render_top(window, cx);
        let detail_width = self.detail_width(window);
        let nav_space = if self.nav_open {
            self.prefs.nav_width
        } else {
            SHEET_MARGIN
        };
        let detail_space = if self.details_open { detail_width } else { 0. };
        let list_width =
            f32::from(window.viewport_size().width) - nav_space - detail_space - SHEET_MARGIN;
        self.list_wide = list_width > LIST_COLUMNS_MIN_WIDTH;
        self.list_refs = list_width > LIST_REFS_MIN_WIDTH;
        let list_el = self.render_list(window, cx);
        let detail = self
            .details_open
            .then(|| self.render_detail(detail_width, window, cx));
        let menu = self.render_menu(window, cx);
        let composer = self.render_composer(window, cx);
        let toast = self.render_toast(cx);
        let nav_edge = self.prefs.nav_width;
        let split = self.split_drag;
        let nav_open = self.nav_open;
        let details_open = self.details_open;
        let nav_handle = self.split_handle(Split::Nav, cx);
        let detail_handle = self.split_handle(Split::Detail, cx);
        measured(div(), "app")
            .relative()
            .size_full()
            .flex()
            .bg(t.chrome)
            .text_color(t.ink)
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .line_height(px(UI_LINE_HEIGHT))
            .key_context(key_context)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::toggle_details))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::mark_done))
            .on_action(cx.listener(Self::cancel_task))
            .on_action(cx.listener(Self::change_status))
            .on_action(cx.listener(Self::change_priority))
            .on_action(cx.listener(Self::edit_title))
            .on_action(cx.listener(Self::copy_ref))
            .on_action(cx.listener(Self::new_task))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::close_window))
            .when_some(split, |root, split| {
                root.cursor_col_resize()
                    .on_mouse_move(
                        cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                            this.drag_split(split, f32::from(event.position.x), window, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            this.split_drag = None;
                            if let Err(error) = this.prefs.save(&this.prefs_path) {
                                eprintln!("aven-desktop: {error:#}");
                            }
                            cx.notify();
                        }),
                    )
            })
            .when(self.thumb_drag.is_some(), |root| {
                root.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    let Some((start, offset)) = this.thumb_drag else {
                        return;
                    };
                    let viewport = this.list.viewport_bounds().size.height;
                    let max = this.list.max_offset_for_scrollbar().y;
                    let ratio = (max + viewport) / viewport;
                    let next = (offset + (event.position.y - start) * ratio).clamp(px(0.), max);
                    this.list.set_offset_from_scrollbar(point(px(0.), -next));
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.thumb_drag = None;
                        this.list.scrollbar_drag_ended();
                        cx.notify();
                    }),
                )
            })
            .when_some(nav, |root, nav| root.child(nav))
            .child(
                measured(div(), "main")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .mt(px(SHEET_MARGIN))
                    .mr(px(SHEET_MARGIN))
                    .mb(px(SHEET_MARGIN))
                    .ml(px(if nav_open {
                        -SPLIT_OVERLAP
                    } else {
                        SHEET_MARGIN
                    }))
                    .rounded(px(10.))
                    .bg(t.page)
                    .shadow(t.sheet_shadow())
                    .overflow_hidden()
                    .child(top)
                    .child(
                        div().flex_1().min_h_0().flex().child(list_el).when_some(
                            detail,
                            |body, detail| {
                                body.child(div().flex().flex_none().h_full().child(detail))
                            },
                        ),
                    ),
            )
            .when(nav_open, |root| {
                root.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(nav_edge - 3.))
                        .child(nav_handle),
                )
            })
            .when(details_open, |root| {
                root.child(
                    div()
                        .absolute()
                        .top(px(50.))
                        .bottom(px(SHEET_MARGIN))
                        .right(px(SHEET_MARGIN + detail_width - 4.))
                        .w(px(7.))
                        .child(detail_handle),
                )
            })
            .when_some(toast, |root, toast| root.child(toast))
            .when_some(menu, |root, menu| root.child(menu))
            .when_some(composer, |root, composer| root.child(composer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(name: &str) -> Item {
        Item::Section {
            index: 0,
            name: name.to_string(),
            hint: String::new(),
            count: 0,
            height: 32.,
        }
    }

    fn row(id: &TaskId) -> Item {
        Item::Row {
            task: 0,
            row: 0,
            id: id.clone(),
        }
    }

    #[test]
    fn a_move_glides_the_rows_it_shifts_and_fades_the_groups_it_changes() {
        let (first, second) = (TaskId::new(), TaskId::new());
        let old = [section("Focus"), row(&first), row(&second)];
        let new = [section("Focus"), row(&second), section("Done"), row(&first)];
        let shifted = [row(&first), section("Focus"), row(&second)];
        let motion = ListMotion::default().between(&old, &shifted, 0., 500., || 1);
        assert_eq!(motion.section("Focus").unwrap().0, -ROW_HEIGHT);

        let mut runs = 0;
        let motion = ListMotion::default().between(&old, &new, 0., 500., || {
            runs += 1;
            runs
        });
        assert_eq!(motion.row(&second).unwrap().0, ROW_HEIGHT);
        assert_eq!(motion.row(&first).unwrap().0, -(ROW_HEIGHT + 32.));
        assert!(motion.entered("Done").is_some());
        assert!(motion.entered("Focus").is_none());
        assert_eq!(motion.section("Focus"), None);

        let motion = ListMotion::default().between(&new, &old, 10., 500., || 0);
        assert_eq!(motion.left[0].0.name, "Done");
        assert_eq!(motion.left[0].0.top, 32. + ROW_HEIGHT - 10.);
    }

    #[test]
    fn a_move_past_a_screen_lands_at_once() {
        let (first, second) = (TaskId::new(), TaskId::new());
        let old = [row(&first), row(&second)];
        let new = [row(&second), row(&first)];
        let motion = ListMotion::default().between(&old, &new, 0., ROW_HEIGHT - 1., || 0);
        assert_eq!(motion.row(&first), None);
    }

    #[test]
    fn a_scroll_position_lands_in_the_entry_that_holds_it() {
        let id = TaskId::new();
        let items = [Item::Pad(4.), section("Focus"), row(&id)];
        let at = list_offset_at(&items, 40.);
        assert_eq!((at.item_ix, at.offset_in_item), (2, px(4.)));
    }
}

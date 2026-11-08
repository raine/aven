//! The window: navigation, the queue list, and the state that spans them.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use accesskit::Role;
use aven_core::choices::TaskStatus;
use aven_core::ids::TaskId;
use aven_host::store::TaskQuery;
use gpui::{
    Animation, AnimationExt, AnyElement, App, ClipboardItem, Context, Div, Entity, FocusHandle,
    Focusable, FontFeatures, FontWeight, KeyContext, ListAlignment, ListOffset, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, Stateful,
    Subscription, Window, WindowBounds, div, ease_in_out, linear_color_stop, linear_gradient, list,
    point, prelude::*, relative, svg,
};

use crate::actions::*;
use crate::adapter::{
    Bulk, Command, DateTone, DeletedFilter, KnownDatabase, LedgerLine, Note, Prefs, Replies, Reply,
    RowDate, SavedDrafts, SavedWindow, TaskRow, ThemePref, WindowMode, supports_columns,
};
use crate::bounce::{Bounce, ListBounce};
use crate::commands::view_name;
use crate::composer::Composer;
use crate::dialogs::Layer;
use crate::display::DisplayPop;
use crate::draft::Draft;
use crate::input::{Editor, EditorEvent, Mode};
use crate::intro::Playback;
use crate::kit::*;
use crate::lazy_row::LazyRow;
use crate::ledger::{LEDGER_BURST_HEIGHT, LEDGER_DAY_HEIGHT, LEDGER_ROW_HEIGHT};
use crate::markdown;
use crate::overlays::Menu;
use crate::platform;
use crate::session::{
    Applied, PRIORITY_DURATION, PULSE_DURATION, SETTLE_DURATION, Session, SyncTone, count_text,
};
use crate::settings::sync_icon;
use crate::theme::{Theme, mix, theme};
use crate::toast::{Pause, Subject, Toast, ToastAction};
use crate::tooltip::{Hint, Hinted, epic_hint, priority_hint, status_hint};
use crate::welcome::Welcome;
use crate::workflows::{ALL_CLEAR_HEIGHT, LIFECYCLE_HEIGHT, SERIES_HEAD_HEIGHT, SERIES_ROW_HEIGHT};

/// How long a quit waits for the store before giving up on it. Writes queued
/// before the quit decide when it ends, and each can wait on another writer's
/// lock. The window is already gone while it waits.
const QUIT_BUDGET: Duration = Duration::from_secs(40);
const NAV_WIDTH_RANGE: (f32, f32) = (170., 340.);
const DETAIL_WIDTH_RANGE: (f32, f32) = (290., 620.);
/// Resize handles overlap each neighbouring panel by this much.
const SPLIT_OVERLAP: f32 = 1.;
/// The list keeps at least this width when panels are wide.
const LIST_MIN_WIDTH: f32 = 300.;
const SHEET_MARGIN: f32 = 6.;
/// At or below this list width, rows drop their labels and reasons.
const LIST_COLUMNS_MIN_WIDTH: f32 = 680.;
/// At or below this list width, rows also drop their refs, tags and dates.
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
const COMPACT_ROW_HEIGHT: f32 = 26.;

fn row_height() -> f32 {
    if compact() {
        COMPACT_ROW_HEIGHT
    } else {
        ROW_HEIGHT
    }
}
const GLIDE_DURATION: Duration = Duration::from_millis(100);
/// The farthest the highlight glides, enough for a neighbour across a section
/// header. A longer jump lands at once.
const GLIDE_REACH: f32 = 70.;
/// How long a row takes to reach its new place, and a group to fade.
const MOVE_DURATION: Duration = Duration::from_millis(200);
const LEAVE_DURATION: Duration = Duration::from_millis(640);

/// How many of the views the sidebar shows before More views.
const PRIMARY_VIEWS: usize = 9;
/// What Overdue says it lists when the due digest opened it.
const DUE_TODAY_SUBTITLE: &str = "Due today or earlier, as in the daily digest";

/// The name a view's count goes by in the snapshot.
fn count_key(query: TaskQuery) -> &'static str {
    match query {
        TaskQuery::Queue => "queue",
        TaskQuery::Ready => "ready",
        TaskQuery::Inbox => "inbox",
        TaskQuery::Upcoming => "upcoming",
        TaskQuery::Overdue => "overdue",
        TaskQuery::Blocked => "blocked",
        TaskQuery::Epics => "epics",
        TaskQuery::Recurring => "recurring",
        TaskQuery::Conflicts => "conflicts",
        TaskQuery::Active => "active",
        _ => "",
    }
}

/// GPUI snaps lengths to device pixels. Headings carry the remainder from one
/// to the next, so the rows below them do not drift down the list.
fn snapped_height(height: f32) -> f32 {
    (height * 2.).round() / 2.
}

#[derive(Clone, PartialEq)]
enum Item {
    Pad(f32),
    Section {
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
    /// A line of the Recent actions ledger.
    Log {
        line: usize,
        height: f32,
    },
    Empty,
    AllClear,
    Lifecycle,
    SeriesHead,
    Series(usize),
}

impl Item {
    fn height(&self) -> f32 {
        match self {
            Self::Pad(height) | Self::Section { height, .. } | Self::Log { height, .. } => *height,
            Self::Row { .. } => row_height(),
            Self::Empty => 0.,
            Self::AllClear => ALL_CLEAR_HEIGHT,
            Self::Lifecycle => LIFECYCLE_HEIGHT,
            Self::SeriesHead => SERIES_HEAD_HEIGHT,
            Self::Series(_) => SERIES_ROW_HEIGHT,
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
        self.from + (self.to - self.from) * (1. - (1. - delta).powi(4))
    }

    fn top_now(self) -> f32 {
        let delta = self.at.elapsed().as_secs_f32() / GLIDE_DURATION.as_secs_f32();
        self.top(delta.min(1.))
    }
}

/// A held key keeps the highlight within `reach` of the row it is heading for.
fn close_behind(from: f32, to: f32, reach: f32) -> f32 {
    from.clamp(to - reach, to + reach)
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

/// A row on its way off the list.
#[derive(Clone)]
struct LeftRow {
    title: String,
    reference: String,
    project_key: String,
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
    /// Rows that went, drawn where they were until they have collapsed.
    left_rows: Vec<(LeftRow, usize)>,
}

impl ListMotion {
    fn live(&self) -> bool {
        self.at.is_some_and(|at| at.elapsed() < MOVE_DURATION)
    }

    /// Row leaves outlast the glide, so they only stop on their own clock.
    fn left_rows(&self) -> Vec<(LeftRow, usize)> {
        let live = self.at.is_some_and(|at| at.elapsed() < LEAVE_DURATION);
        if live {
            self.left_rows.clone()
        } else {
            Vec::new()
        }
    }

    /// Records rows that left, from where the old list had them.
    fn take_left(
        &mut self,
        old: &[Item],
        left: &[TaskRow],
        scrolled: f32,
        viewport: f32,
        mut next_run: impl FnMut() -> usize,
    ) {
        let mut top = 0.;
        for item in old {
            if let Item::Row { id, .. } = item
                && let Some(row) = left.iter().find(|row| row.id == *id)
                && top - scrolled + row_height() >= 0.
                && top - scrolled <= viewport
            {
                let row = LeftRow {
                    title: row.title.clone(),
                    reference: row.reference.clone(),
                    project_key: row.project_key.clone(),
                    top: top - scrolled,
                };
                self.left_rows.push((row, next_run()));
            }
            top += item.height();
        }
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

/// A description being written, and the stored text it started from.
pub(crate) struct BodyEdit {
    pub task: TaskId,
    pub base: String,
    /// A description that arrived from elsewhere during the edit.
    pub incoming: Option<String>,
}

pub struct Root {
    pub(crate) focus: FocusHandle,
    pub(crate) session: Session,
    items: Vec<Item>,
    list: ListState,
    bounce: Rc<RefCell<Bounce>>,
    glide: Option<Glide>,
    /// The switch toggled last, with a run and when it was toggled.
    pub(crate) switched: crate::switch::Switch,
    /// Whether the detail byline shows the exact creation time.
    pub(crate) created_exact: bool,
    list_motion: ListMotion,
    pub(crate) details_open: bool,
    nav_open: bool,
    narrow: Option<bool>,
    /// Whether the list has room for its optional columns.
    pub(crate) list_wide: bool,
    /// Whether rows have room for refs and tags.
    list_refs: bool,
    /// Whether any task in the list has a date for the date column.
    list_dues: bool,
    pub(crate) prefs: Prefs,
    prefs_path: PathBuf,
    pub(crate) new_label: Entity<Editor>,
    split_drag: Option<Split>,
    pub(crate) title: Entity<Editor>,
    /// The task the title editor holds, which may not be the selection yet.
    title_task: Option<TaskId>,
    /// The stored title the editor last showed. Only a difference from it is an
    /// edit, so a blur never writes back a title that changed elsewhere.
    title_base: String,
    /// When the timer running for the toast fires.
    toast_timer: Option<Instant>,
    pub(crate) description: Entity<Editor>,
    pub(crate) note: Entity<Editor>,
    pub(crate) kid_input: Entity<Editor>,
    pub(crate) note_views: Vec<(Note, Entity<Editor>)>,
    pub(crate) detail_scroll: ScrollHandle,
    pub(crate) menu: Option<Menu>,
    pub(crate) display: Option<DisplayPop>,
    pub(crate) composer: Entity<Composer>,
    pub(crate) dialog: Option<Layer>,
    pub(crate) body: Entity<Editor>,
    pub(crate) body_edit: Option<BodyEdit>,
    /// The keys of a chord under way, for the hint bar.
    chord: Option<String>,
    more_views: bool,
    pub(crate) anchors: Anchors,
    last_offset: Pixels,
    last_scroll: Option<Instant>,
    thumb_drag: Option<(Pixels, Pixels)>,
    subscriptions: Vec<Subscription>,
    /// Takes every key while the launcher shortcut is being recorded.
    pub(crate) recorder: Option<Subscription>,
    /// Why the last recorded launcher shortcut was not taken.
    pub(crate) shortcut_error: Option<String>,
    /// The Quick capture page shows the agent's command.
    pub(crate) agent_open: bool,
    quitting: bool,
    pub(crate) welcome: Option<Welcome>,
    /// The welcome's intro, which plays the first time the welcome appears.
    pub(crate) intro: Playback,
}

impl Root {
    pub fn new(
        session: Session,
        mut replies: Replies,
        prefs: Prefs,
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
                "Add a note. Return saves, Shift+Return adds a line.",
                window,
                cx,
            )
            .max_height(px(206.))
        });
        let kid_input =
            cx.new(|cx| Editor::new(Mode::Single, "Add a task to this epic", window, cx));
        let body = cx.new(|cx| {
            Editor::new(
                Mode::Multi,
                "Add a description. Markdown works.",
                window,
                cx,
            )
            .enter_breaks()
            .takes_images()
        });
        let subscriptions = vec![
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.subscribe_in(&note, window, Self::on_note_event),
            cx.subscribe_in(&kid_input, window, Self::on_kid_event),
            cx.subscribe_in(&body, window, Self::on_body_event),
            cx.observe_window_activation(window, |this, window, cx| {
                // Popups do not outlive the window's focus, as native ones do not.
                if !window.is_window_active() {
                    this.menu = None;
                }
                this.session
                    .pause_toast(Pause::Away, !window.is_window_active());
                cx.notify();
            }),
            cx.observe_pending_input(window, |this, window, cx| {
                this.show_chord(window, cx);
            }),
            cx.observe_keystrokes(|this, event, window, cx| {
                this.after_keystroke(event.action.is_some(), window, cx);
            }),
        ];
        let focus = cx.focus_handle();
        set_scale(prefs.zoom, prefs.compact);
        let drafts = SavedDrafts::load(&Prefs::drafts_path(&prefs_path));
        let mut session = session;
        session.note_drafts.extend(
            drafts
                .notes
                .into_iter()
                .filter_map(|(id, text)| Some((id.parse().ok()?, text))),
        );
        let new_label = cx.new(|cx| Editor::new(Mode::Single, "New label", window, cx));
        let composer = cx.new(|_| {
            Composer::new(
                drafts.composer.map(Draft::from).unwrap_or_default(),
                crate::composer::Source::from_session(&session),
                focus.clone(),
            )
        });
        let mut subscriptions = subscriptions;
        subscriptions.push(cx.subscribe_in(&composer, window, Self::on_composer_request));
        subscriptions.push(cx.observe(&composer, |_, _, cx| cx.notify()));
        let mut this = Self {
            focus,
            session,
            items: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(300.)).measure_all(),
            bounce: Rc::default(),
            glide: None,
            switched: Default::default(),
            created_exact: false,
            list_motion: ListMotion::default(),
            details_open: prefs.details_open,
            nav_open: prefs.nav_open,
            narrow: None,
            list_wide: true,
            list_refs: true,
            list_dues: true,
            prefs,
            new_label,
            prefs_path,
            split_drag: None,
            title,
            title_task: None,
            title_base: String::new(),
            toast_timer: None,
            description,
            note,
            kid_input,
            note_views: Vec::new(),
            detail_scroll: ScrollHandle::new(),
            menu: None,
            display: None,
            composer,
            dialog: None,
            body,
            body_edit: None,
            chord: None,
            more_views: false,
            anchors: Rc::default(),
            last_offset: px(0.),
            last_scroll: None,
            thumb_drag: None,
            subscriptions,
            recorder: None,
            shortcut_error: None,
            agent_open: false,
            quitting: false,
            welcome: None,
            intro: Playback::default(),
        };
        this.rebuild(false, &[]);
        this.load_selection(window, cx);
        window.focus(&this.focus, cx);
        // Apply data already loaded during native initialization before drawing.
        // Bound the drain so a busy producer cannot hold up the first frame.
        for _ in 0..replies.len() {
            if let Ok(reply) = replies.try_recv() {
                this.take_reply(reply, window, cx);
            }
        }
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
                // Back from System Settings, perhaps with Login Items or
                // notifications changed.
                if window.is_window_active() {
                    this.refresh_settings(cx);
                }
            }));
    }

    fn sync_system(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reduced = platform::reduce_motion();
        cx.set_reduce_motion(reduced);
        self.session.motion.set_reduced(reduced);
        self.apply_theme(window, cx);
    }

    pub(crate) fn apply_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let dark = match self.prefs.theme {
            ThemePref::Dark => true,
            ThemePref::Light => false,
            ThemePref::System => matches!(
                window.appearance(),
                gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
            ),
        };
        if !cx.has_global::<Theme>() || theme(cx).dark != dark {
            cx.set_global(if dark { Theme::dark() } else { Theme::light() });
            self.tint_composer_title(cx);
        }
        cx.notify();
    }

    pub(crate) fn apply_scale(&mut self, cx: &mut Context<Self>) {
        set_scale(self.prefs.zoom, self.prefs.compact);
        self.rebuild(false, &[]);
        cx.notify();
    }

    pub(crate) fn save_prefs(&self) {
        if let Err(error) = self.prefs.save(&self.prefs_path) {
            eprintln!("aven-desktop: {error:#}");
        }
    }

    /// Keeps what the next launch restores, in case this run never quits cleanly.
    fn save_state(&mut self, window: &Window, cx: &App) {
        let (bounds, mode) = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => (bounds, WindowMode::Windowed),
            WindowBounds::Maximized(bounds) => (bounds, WindowMode::Zoomed),
            WindowBounds::Fullscreen(bounds) => (bounds, WindowMode::FullScreen),
        };
        self.prefs.window = Some(SavedWindow {
            display: window
                .display(cx)
                .and_then(|display| display.uuid().ok())
                .map(|uuid| uuid.to_string()),
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            mode,
        });
        self.prefs.nav_open = self.nav_open;
        if self.narrow == Some(false) {
            self.prefs.details_open = self.details_open;
        }
        self.prefs.view = Some(self.session.saved_view());
        self.save_prefs();
        self.keep_note_draft(cx);
        self.save_drafts(cx);
    }

    fn save_drafts(&self, cx: &App) {
        let draft = self.composer.read(cx).saved_draft(cx);
        let mut notes: HashMap<String, String> = self
            .session
            .note_drafts
            .iter()
            .map(|(id, text)| (id.to_string(), text.clone()))
            .collect();
        // A description waiting on an incoming change cannot be committed, so
        // its text comes back as a note on the task.
        if let Some(edit) = self
            .body_edit
            .as_ref()
            .filter(|edit| edit.incoming.is_some())
        {
            let note = notes.entry(edit.task.to_string()).or_default();
            if !note.is_empty() {
                note.push_str("\n\n");
            }
            note.push_str(self.description.read(cx).text());
        }
        let drafts = SavedDrafts {
            composer: (draft != Draft::default()).then(|| (&draft).into()),
            notes: notes.into_iter().collect(),
        };
        if let Err(error) = drafts.save(&Prefs::drafts_path(&self.prefs_path)) {
            eprintln!("aven-desktop: {error:#}");
        }
    }

    /// Takes the window off screen with everything in it left running. The Dock
    /// icon and Show Aven bring it back.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_state(window, cx);
        // A full screen window owns its space, so ordering it out leaves that blank.
        if window.is_fullscreen() {
            cx.hide();
        } else {
            platform::order_out(window);
        }
    }

    /// Saves what is unfinished and asks the store to wind down. Returns false
    /// if a quit is already under way.
    pub fn begin_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if std::mem::replace(&mut self.quitting, true) {
            return false;
        }
        self.commit_title(cx);
        let waiting = self
            .body_edit
            .as_ref()
            .is_some_and(|edit| edit.incoming.is_some());
        if !waiting {
            self.commit_description(cx);
        }
        self.save_state(window, cx);
        self.session.quit();
        true
    }

    /// Quits once the store has wound down, or after `QUIT_BUDGET` if it cannot.
    pub fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.begin_quit(window, cx) {
            return;
        }
        platform::order_out(window);
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(QUIT_BUDGET).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    /// Every database stays known, so one in use is there after a relaunch.
    fn database_opened(&mut self, opened: KnownDatabase, previous: Option<KnownDatabase>) {
        for known in [Some(opened), previous].into_iter().flatten() {
            self.prefs
                .databases
                .retain(|other| other.path != known.path);
            self.prefs.databases.push(known);
        }
        self.save_prefs();
    }

    fn build_items(&self) -> Vec<Item> {
        let snapshot = &self.session.snapshot;
        let mut items = vec![Item::Pad(4.)];
        if snapshot.view.query == TaskQuery::RecentActions {
            items.extend(
                snapshot
                    .recent
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(line, entry)| {
                        let height = match entry {
                            LedgerLine::Day { .. } => LEDGER_DAY_HEIGHT,
                            LedgerLine::Burst { .. } => LEDGER_BURST_HEIGHT,
                            LedgerLine::Run(_) => LEDGER_ROW_HEIGHT,
                        };
                        Item::Log { line, height }
                    }),
            );
            if items.len() == 1 {
                items.push(Item::Empty);
            }
            items.push(Item::Pad(40.));
            return items;
        }
        if snapshot.view.query == TaskQuery::Recurring {
            items.push(Item::Lifecycle);
            if snapshot.series.is_empty() {
                items.push(Item::Empty);
            } else {
                items.push(Item::SeriesHead);
                items.extend((0..snapshot.series.len()).map(Item::Series));
            }
            items.push(Item::Pad(40.));
            return items;
        }
        if self.session.all_clear() {
            items.push(Item::AllClear);
        }
        let mut row = 0;
        let mut carried = 0.;
        for group in &snapshot.groups {
            let height = snapped_height(SECTION_HEIGHT + carried);
            carried += SECTION_HEIGHT - height;
            // A view core does not group lists its rows straight away.
            if !group.name.is_empty() {
                items.push(Item::Section {
                    name: group.name.clone(),
                    hint: group.hint.clone(),
                    count: group.rows.len(),
                    height,
                });
            }
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
    fn rebuild(&mut self, moved: bool, left: &[TaskRow]) {
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
                    self.list_motion
                        .take_left(&self.items, left, scrolled, viewport, || motion.next_run());
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
        if let Some((opened, previous)) = applied.opened {
            self.database_opened(opened, previous);
        }
        if applied.list {
            self.rebuild(applied.moved.is_some(), &applied.left);
            if !applied.left.is_empty() {
                self.repaint_after(LEAVE_DURATION, cx);
            }
            if self.session.motion.ringing() {
                self.repaint_after(crate::session::ALL_CLEAR_DURATION, cx);
            }
        }
        if let Some(id) = applied.stop {
            self.stop_series(id, window, cx);
        }
        if let Some(task) = applied.composer_draft {
            self.return_failed_draft(Draft::from_task(*task), window, cx);
        }
        if applied.stopped {
            // A write that failed during the quit has handed its text back.
            self.save_drafts(cx);
            cx.quit();
        }
        if let Some(failed) = applied.unreviewed_failed {
            self.unreviewed_failed(failed, cx);
        }
        if let Some(text) = applied.copy {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if let Some(catalog) = applied.launcher {
            cx.defer(move |cx| crate::launcher::set_catalog(*catalog, cx));
        }
        if let Some(event) = applied.launch {
            self.take_launch_event(event, cx);
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
        cx.notify();
    }

    /// The key of the workspace the window shows.
    pub(crate) fn active_workspace_key(&self) -> Option<&str> {
        self.session.active_workspace_key()
    }

    /// Says in the window that the launcher saved a task, for when macOS
    /// holds its notification back.
    pub(crate) fn launched(
        &mut self,
        title: &str,
        created: &crate::launch::Created,
        cx: &mut Context<Self>,
    ) {
        let here = self.active_workspace_key() == Some(created.workspace.as_str());
        let go = here.then(|| {
            ToastAction::Go(crate::adapter::Go {
                label: "View task",
                command: Command::Goto(created.task.clone()),
            })
        });
        let detail = format!("{} \u{b7} {}", created.reference, created.title);
        self.session
            .show_toast(Toast::done(title).detail(detail).action(go));
        cx.notify();
    }

    /// Shows a task a launcher notification was about, in its workspace.
    pub(crate) fn open_launched(
        &mut self,
        workspace: String,
        id: TaskId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_workspace_key() == Some(workspace.as_str()) {
            self.session.goto(id);
        } else {
            self.switch_view(Command::SwitchWorkspace(workspace), window, cx);
            self.session.switch_view(Command::Goto(id));
        }
        cx.notify();
    }

    /// Shows the tasks due today and earlier in `workspace`.
    pub(crate) fn open_due(
        &mut self,
        workspace: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_workspace_key() == Some(workspace.as_str()) {
            self.switch_view(Command::ShowDue, window, cx);
        } else {
            self.switch_view(Command::SwitchWorkspace(workspace), window, cx);
            self.session.switch_view(Command::ShowDue);
        }
        cx.notify();
    }

    /// Keeps a timer running for the toast's deadline. Every frame checks, so
    /// a toast shown from anywhere expires without its caller arming it.
    fn arm_toast(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.session.toasts.wake() else {
            return;
        };
        if self.toast_timer.replace(deadline) == Some(deadline) {
            return;
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                if this.session.expire_toast() {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn run_toast_action(
        &mut self,
        _: &RunToastAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.session.run_toast_action() {
            Some(ToastAction::RestoreDraft) => self.restore_draft(window, cx),
            Some(ToastAction::OpenDraft(job)) => self.open_kept_draft(job, window, cx),
            Some(ToastAction::Open(url)) => cx.open_url(&url),
            _ => {}
        }
        cx.notify();
    }

    /// Brings open editors up to date with stored data. Focused editors keep
    /// their text, with any IME composition inside it.
    fn sync_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.session.target_row().map(|row| row.title.clone());
        if let Some(title) = title
            && !self.title.read(cx).focus.is_focused(window)
        {
            self.show_title(title, cx);
        }
        let Some(detail) = self.session.detail.clone() else {
            return;
        };
        if self.title_task.is_none() {
            self.title_task = Some(detail.task_id.clone());
        }
        if let Some(edit) = self
            .body_edit
            .as_mut()
            .filter(|edit| edit.task == detail.task_id)
        {
            let moved = detail.description != edit.base;
            edit.incoming = moved.then(|| detail.description.clone());
        }
        let rendered = markdown::render(&detail.description);
        if !self.description.read(cx).shows(&rendered) {
            self.description
                .update(cx, |editor, cx| editor.set_rendered(rendered, cx));
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
        let rendered = markdown::render(text);
        cx.new(|cx| {
            let mut editor = Editor::new(Mode::Multi, "", window, cx)
                .read_only()
                .paragraph_gap(px(8.));
            editor.set_rendered(rendered, cx);
            editor
        })
    }

    fn load_note_draft(&mut self, cx: &mut Context<Self>) {
        let draft = self
            .session
            .target()
            .and_then(|id| self.session.note_drafts.get(id))
            .cloned()
            .unwrap_or_default();
        self.note.update(cx, |note, cx| note.set_text(&draft, cx));
    }

    /// Points the details editors at the current selection.
    pub(crate) fn load_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.session.target_row();
        self.title_task = self.session.target().cloned();
        let title = row.map(|row| row.title.clone()).unwrap_or_default();
        self.show_title(title, cx);
        self.load_note_draft(cx);
        self.note_views.clear();
        self.description
            .update(cx, |editor, cx| editor.set_text("", cx));
        self.detail_scroll.set_offset(point(px(0.), px(0.)));
        self.sync_editors(window, cx);
    }

    pub(crate) fn commit_title(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.title_task.clone() else {
            return;
        };
        let text = self.title.read(cx).text().trim().to_string();
        if text == self.title_base {
            return;
        }
        if text.is_empty() {
            let saved = self.title_base.clone();
            self.show_title(saved, cx);
        } else {
            self.title_base = text.clone();
            self.session.set_title(&id, text);
        }
    }

    pub(crate) fn keep_note_draft(&mut self, cx: &App) {
        let Some(id) = self.session.target().cloned() else {
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
        self.commit_description(cx);
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

    pub(crate) fn list_to_top(&self) {
        self.list.scroll_to(ListOffset::default());
    }

    pub(crate) fn is_narrow(&self) -> bool {
        self.narrow == Some(true)
    }

    fn show_chord(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = window.pending_input_keystrokes();
        let keys = pending
            .and_then(|keys| keys.first())
            .map(|key| key.key.clone());
        if keys.is_some() {
            self.chord = keys;
            cx.notify();
        }
    }

    /// Ends the hint bar, saying so when the key finished no command.
    fn after_keystroke(&mut self, handled: bool, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_pending_keystrokes() || self.chord.is_none() {
            return;
        }
        self.chord = None;
        if !handled {
            self.session
                .show_toast(Toast::info("No command on that key"));
        }
        cx.notify();
    }

    fn reveal_log(&self) {
        let key = self.session.log.as_ref();
        let line = self
            .session
            .snapshot
            .recent
            .lines
            .iter()
            .position(|line| matches!(line, LedgerLine::Run(run) if Some(&run.key) == key));
        let item = line.and_then(|line| {
            self.items
                .iter()
                .position(|item| matches!(item, Item::Log { line: at, .. } if *at == line))
        });
        if let Some(item) = item {
            self.list.scroll_to_reveal_item(item);
        }
    }

    pub(crate) fn reveal_selection(&self) {
        if let Some(position) = self
            .session
            .selected
            .as_ref()
            .and_then(|id| self.item_of(id))
        {
            let viewport = f32::from(self.list.viewport_bounds().size.height);
            if viewport > 0. {
                let offset =
                    selection_scroll_offset(&self.items, position, self.list_offset(), viewport);
                self.list.scroll_to(list_offset_at(&self.items, offset));
            } else {
                self.list.scroll_to_reveal_item(position);
            }
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
        let moving = self.glide.filter(|glide| glide.live()).map(Glide::top_now);
        let trailing = moving.is_some();
        let from = moving
            .or_else(|| {
                self.session
                    .selected
                    .as_ref()
                    .and_then(|sel| self.row_top(sel))
            })
            .filter(|top| (offset - row_height()..offset + viewport).contains(top));
        let target = from.zip(self.row_top(id));
        let near = target.filter(|(from, to)| (from - to).abs() <= GLIDE_REACH);
        self.glide = near.map(|(from, to)| Glide {
            from: if trailing {
                close_behind(from, to, row_height())
            } else {
                from
            },
            to,
            run: self.session.motion.next_run(),
            at: Instant::now(),
        });
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.step_child(delta, cx) {
            return;
        }
        if self.session.snapshot.view.query == TaskQuery::Recurring {
            if let Some(id) = self.session.neighbour_series(delta) {
                self.session.select_series(id);
                cx.notify();
            }
        } else if self.session.snapshot.view.query == TaskQuery::RecentActions {
            if let Some(key) = self.session.neighbour_log(delta) {
                self.session.log = Some(key);
                self.reveal_log();
                cx.notify();
            }
        } else if let Some(id) = self.session.neighbour(delta) {
            self.select(id, window, cx);
        }
    }

    fn move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    fn move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, window, cx);
    }

    pub(crate) fn toggle_theme(
        &mut self,
        _: &ToggleTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prefs.theme = if theme(cx).dark {
            ThemePref::Light
        } else {
            ThemePref::Dark
        };
        self.save_prefs();
        self.apply_theme(window, cx);
    }

    pub(crate) fn toggle_details(
        &mut self,
        _: &ToggleDetails,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.details_open = !self.details_open;
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.nav_open = !self.nav_open;
        cx.notify();
    }

    pub(crate) fn set_status(
        &mut self,
        action: &SetStatus,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.target_id() else {
            return;
        };
        if !self.session.bulk(Bulk::Status(action.0)) {
            self.session.set_status(&id, action.0);
        }
        self.after_status_motion(cx);
    }

    pub(crate) fn mark_done(&mut self, _: &MarkDone, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.bulk(Bulk::Status(TaskStatus::Done)) {
            cx.notify();
        } else if let Some(id) = self.session.child.clone() {
            self.session.set_status(&id, TaskStatus::Done);
            cx.notify();
        } else if let Some(id) = self.session.target().cloned() {
            self.session.toggle_status(&id, TaskStatus::Done);
            self.after_status_motion(cx);
        }
    }

    pub(crate) fn cancel_task(&mut self, _: &CancelTask, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.bulk(Bulk::Status(TaskStatus::Canceled)) {
            cx.notify();
        } else if let Some(id) = self.session.child.clone() {
            self.session.set_status(&id, TaskStatus::Canceled);
            cx.notify();
        } else if let Some(id) = self.session.target().cloned() {
            self.session.toggle_status(&id, TaskStatus::Canceled);
            self.after_status_motion(cx);
        }
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
        let Some(row) = self.session.target_row() else {
            return;
        };
        let subject = Some(Subject {
            reference: row.reference.clone(),
            project_key: row.project_key.clone(),
        });
        cx.write_to_clipboard(ClipboardItem::new_string(row.reference.clone()));
        self.session
            .show_toast(Toast::done("Copied").subject(subject));
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
                let saved = self.title_base.clone();
                self.show_title(saved, cx);
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
                let Some(id) = self.session.target().cloned().filter(|_| !text.is_empty()) else {
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

    pub(crate) fn close_window(
        &mut self,
        _: &CloseWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
    }

    pub(crate) fn minimize(&mut self, _: &Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window();
    }

    pub(crate) fn zoom(&mut self, _: &Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window();
    }

    pub(crate) fn toggle_full_screen(
        &mut self,
        _: &ToggleFullScreen,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.toggle_fullscreen();
    }

    /// An entry for the list. Rows wait until they are painted, because
    /// `measure_all` asks for every entry and a row's height is fixed.
    fn render_entry(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !matches!(self.items[ix], Item::Row { .. }) {
            return self.render_item(ix, window, cx);
        }
        let this = cx.entity();
        LazyRow::new(row_height(), move |window, cx| {
            this.update(cx, |this, cx| this.render_item(ix, window, cx))
        })
        .into_any_element()
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
            Item::Empty => self.render_empty(cx),
            Item::Log { line, .. } => self.render_ledger_line(line, cx),
            Item::AllClear => self.render_all_clear(cx),
            Item::Lifecycle => self.render_lifecycle(cx),
            Item::SeriesHead => self.render_series_head(cx),
            Item::Series(index) => self.render_series_row(index, cx),
            Item::Section {
                name,
                hint,
                count,
                height,
            } => {
                let entered = self.list_motion.entered(&name);
                let moved = self.list_motion.section(&name);
                let section = self.section_heading(div(), name, hint, count, height, &t);
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
            .text(Text::Secondary)
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
                        .text(Text::Caption)
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

    /// The row's title, dimming while the task has just been completed, with
    /// a star after it when the task is an epic.
    fn task_title(
        &self,
        id: &TaskId,
        title: String,
        status: TaskStatus,
        deleted: bool,
        epic: bool,
        t: &Theme,
    ) -> AnyElement {
        let closed = status.is_terminal();
        let (ink, faint) = (t.ink, t.faint);
        let label = div()
            .min_w_0()
            .overflow_hidden()
            .text_color(if closed { faint } else { ink })
            .when(deleted, |label| label.line_through().text_color(faint))
            .child(
                div()
                    .id(gpui::SharedString::from(format!("title-{id}")))
                    .truncate()
                    .hint(Hint::new(title.clone()).when_clipped(Text::Body))
                    .child(title),
            );
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
            .items_center()
            .gap(px(6.))
            .child(body)
            .when(epic, |title| {
                title.child(
                    div()
                        .id(gpui::SharedString::from(format!("star-{id}")))
                        .flex_none()
                        .hint(epic_hint(closed || deleted))
                        .child(epic_star(closed || deleted, t)),
                )
            })
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
        let query_is_epics = self.session.snapshot.view.query == TaskQuery::Epics;
        let data = self.session.snapshot.tasks[task].clone();
        let epic_cells = query_is_epics.then(|| self.epic_cells(&data, cx, &t));
        let id = data.id.clone();
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
        let deleted = data.deleted;
        let shown = self.prefs.row_fields;
        let head = self.render_row_head(ix, row, &data, window, cx, &t);
        let row_ix = row;
        let dates = [
            ("created", "Created", data.created.filter(|_| shown.created)),
            ("updated", "Updated", data.updated.filter(|_| shown.updated)),
        ]
        .into_iter()
        .filter(|_| self.list_refs && !query_is_epics)
        .filter_map(|(key, name, date)| Some(date_cell((key, row), name, date?, &t)));
        let body = head
            .when(self.list_refs, |row| {
                row.child(ref_label(&data.reference, &data.project_key, Some(74.), &t))
            })
            .child(self.task_title(&id, data.title, data.status, deleted, data.is_epic, &t))
            .when(deleted, |row| {
                row.child(
                    small(Text::Secondary)
                        .flex_none()
                        .text_color(t.bad)
                        .child("Deleted"),
                )
            })
            .when(data.recurring, |row| {
                row.child(
                    small(Text::Secondary)
                        .id(("recurring", row_ix))
                        .hint(Hint::new("Recurring task"))
                        .flex_none()
                        .text_color(t.faint)
                        .child("↻"),
                )
            })
            .when_some(data.reason.filter(|_| wide), |row, (reason, bad)| {
                row.child(
                    small(Text::Secondary)
                        .flex_none()
                        .text_color(if bad { t.bad } else { t.warn })
                        .child(reason),
                )
            })
            .when(data.blocked && self.list_refs, |row| {
                row.child(
                    small(Text::Secondary)
                        .flex_none()
                        .text_color(t.warn)
                        .child("Blocked"),
                )
            })
            .when(wide && shown.labels && !labels.is_empty(), |row| {
                row.child(
                    small(Text::Secondary)
                        .flex_none()
                        .max_w(px(112.))
                        .text_right()
                        .truncate()
                        .text_color(t.faint)
                        .child(labels),
                )
            })
            .children(epic_cells.into_iter().flatten())
            .when_some(
                data.progress.filter(|_| !query_is_epics),
                |row, progress| {
                    row.child(
                        div()
                            .flex_none()
                            .w(px(52.))
                            .font_family(MONO)
                            .text(Text::Secondary)
                            .text_color(t.faint)
                            .child(progress),
                    )
                },
            )
            .when(self.list_dues, |row| {
                row.child(
                    small(Text::Secondary)
                        .flex_none()
                        .w(px(64.))
                        .text_right()
                        .text_color(date_color)
                        .child(data.due),
                )
            })
            .children(dates);
        let place = div().px(px(8.)).child(body);
        match self.list_motion.row(&id) {
            Some(start) => glide_to_place(place, start),
            None => place.into_any_element(),
        }
    }

    fn render_row_head(
        &self,
        ix: usize,
        row: usize,
        data: &TaskRow,
        window: &Window,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Stateful<Div> {
        let id = data.id.clone();
        let selected = self.session.selected.as_ref() == Some(&id);
        let below_selected = matches!(
            &self.items[ix - 1],
            Item::Row { id, .. } if self.session.selected.as_ref() == Some(id)
        );
        let active = window.is_window_active();
        let gliding = self.glide.is_some_and(Glide::live);
        let (select_id, toggle_id, menu_id) = (id.clone(), id.clone(), id.clone());
        div()
            .relative()
            .id(("row", row))
            .role(Role::ListItem)
            .aria_label(format!("{} {}", data.reference, data.title))
            .aria_selected(selected)
            .h(px(row_height()))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(9.))
            .rounded(px(RADIUS_CONTROL))
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
                        .rounded(px(RADIUS_CONTROL))
                        .group_hover("below-selected", |style| style.bg(t.panel)),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    let toggle = event.modifiers.platform || event.modifiers.control;
                    let shift = event.modifiers.shift;
                    if this.mark_click(&select_id, shift, toggle, window, cx) {
                        return;
                    }
                    this.select(select_id.clone(), window, cx);
                    if event.click_count == 2 {
                        this.expand(&Expand, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.row_menu(menu_id.clone(), event.position, window, cx);
                }),
            )
            .child(mark_box(self.session.marks.contains(&id), t))
            .child(
                div()
                    .id(("row-priority", row))
                    .flex_none()
                    .hint(priority_hint(data.priority, selected))
                    .child(self.task_priority(
                        &id,
                        data.priority,
                        ("pri-row-old", "pri-row-new"),
                        t,
                    )),
            )
            .child(
                div()
                    .id(("glyph", row))
                    .role(Role::CheckBox)
                    .aria_label("Toggle done")
                    .hint(status_hint(data.status, selected))
                    .flex_none()
                    .size(px(14.))
                    .rounded_full()
                    .hover(|style| style.shadow(vec![spread(t.raised, 3.)]))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.session.toggle_status(&toggle_id, TaskStatus::Done);
                        this.after_status_motion(cx);
                    }))
                    .child(self.task_glyph(&id, data.status, t)),
            )
    }

    fn render_nav(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let active = window.is_window_active();
        let snapshot = &self.session.snapshot;
        let header = |label: &'static str| {
            div()
                .px(px(8.))
                .pt(px(14.))
                .pb(px(4.))
                .text(Text::Caption)
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
            let key = workspace.key.clone();
            body = body.child(
                nav_item(("workspace", ix), &t)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.switch_view(Command::SwitchWorkspace(key.clone()), window, cx);
                    }))
                    .when(on, |item| {
                        item.text_color(t.ink).font_weight(FontWeight::MEDIUM)
                    })
                    .child(workspace.name.clone())
                    .child(nav_count(workspace.count.to_string(), false, &t)),
            );
        }
        body = body.child(header("Views"));
        let view_item = |ix: usize, cx: &mut Context<Self>| {
            let (query, name, icon_path, keys, about) = VIEWS[ix];
            let on = query == snapshot.view.query;
            let key = count_key(query);
            let count = snapshot
                .views
                .iter()
                .find(|(view, _)| *view == key)
                .map_or(0, |(_, count)| *count);
            let hot = matches!(query, TaskQuery::Overdue | TaskQuery::Conflicts) && count > 0;
            nav_item(("view", ix), &t)
                .hint(
                    Hint::new(name)
                        .keys(keys)
                        .when(!about.is_empty(), |hint| hint.sub(about))
                        .beside(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_view(&ShowView(query), window, cx);
                }))
                .when(on, |item| {
                    item.bg(if active { t.tint } else { t.raised })
                        .text_color(t.ink)
                        .font_weight(FontWeight::MEDIUM)
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
                ))
        };
        let conflicts = VIEWS.iter().position(|view| view.0 == TaskQuery::Conflicts);
        for ix in (0..PRIMARY_VIEWS).chain(conflicts) {
            body = body.child(view_item(ix, cx));
        }
        body = body.child(
            nav_item("more", &t)
                .text_color(t.faint)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.more_views = !this.more_views;
                    cx.notify();
                }))
                .child(if self.more_views {
                    "Fewer views"
                } else {
                    "More views"
                }),
        );
        if self.more_views {
            for ix in (PRIMARY_VIEWS..VIEWS.len()).filter(|ix| Some(*ix) != conflicts) {
                body = body.child(view_item(ix, cx));
            }
        }
        let scoped = snapshot.view.project.clone();
        body = body.child(header("Projects")).child(
            nav_item("all-projects", &t)
                .hint(Hint::new("All projects").keys("g a").beside())
                .when(scoped.is_none(), |item| {
                    item.text_color(t.ink).font_weight(FontWeight::MEDIUM)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.all_projects(&AllProjects, window, cx);
                }))
                .child(icon("icons/ic-projects.svg", t.faint))
                .child("All projects"),
        );
        for (ix, project) in snapshot.projects.iter().enumerate() {
            let key = project.key.clone();
            let menu_key = project.key.clone();
            let menu_name = project.name.clone();
            let on = scoped.as_ref() == Some(&project.key);
            body = body.child(
                nav_item(("project", ix), &t)
                    .when(on, |item| {
                        item.text_color(t.ink).font_weight(FontWeight::MEDIUM)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.scope_to(Some(key.clone()), window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.project_menu(
                                menu_key.clone(),
                                menu_name.clone(),
                                event.position,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(project_dot(&project.key, &t).mx(px(4.)))
                    .child(project.name.clone())
                    .child(nav_count(project.count.to_string(), false, &t)),
            );
        }
        let (sync_tone, _) = self.session.sync_line();
        let sync_problem = matches!(sync_tone, SyncTone::Warn | SyncTone::Bad);
        div()
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
                    .flex_wrap()
                    .items_center()
                    .gap(px(2.))
                    .text(Text::Secondary)
                    .line_height(px(16.8))
                    .when_some(self.render_update_row(cx), |footer, row| footer.child(row))
                    .when(sync_problem, |footer| {
                        footer.child(self.render_sync_state(cx).w_full())
                    })
                    .child(
                        nav_item("settings", &t)
                            .role(Role::Button)
                            .aria_label("Settings")
                            .hint(Hint::new("Settings").keys(by_platform!("⌘,", "Ctrl+,")))
                            .w_auto()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(&OpenSettings, window, cx);
                            }))
                            .h(px(28.))
                            .mb_0()
                            .text_color(t.muted)
                            .child(icon("icons/ic-settings.svg", t.faint))
                            .child("Settings"),
                    )
                    .child(div().flex_1())
                    .when(!sync_problem, |footer| {
                        footer.child(self.render_sync_state(cx))
                    }),
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
                    .hint(
                        Hint::new(if self.nav_open {
                            "Hide sidebar"
                        } else {
                            "Show sidebar"
                        })
                        .keys("g b"),
                    )
                    .size(px(26.))
                    .rounded(px(RADIUS_CONTROL))
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
        let active = window.is_window_active();
        let snapshot = &self.session.snapshot;
        let count = if snapshot.view.query == TaskQuery::Recurring {
            snapshot.series.len()
        } else {
            snapshot.ordered().count()
        };
        let view = &snapshot.view;
        let query = view.query;
        let name = if self.session.settings.is_some() {
            "Settings"
        } else if self.session.review.is_some() {
            "Triage"
        } else {
            view_name(query)
        };
        let subtitle = if view.due_today {
            DUE_TODAY_SUBTITLE
        } else {
            VIEWS
                .iter()
                .find(|entry| entry.0 == query)
                .map_or("", |entry| entry.4)
        };
        let scope_name = view
            .project
            .as_ref()
            .and_then(|key| snapshot.projects.iter().find(|project| project.key == *key))
            .map_or("All projects".to_string(), |project| project.name.clone());
        let expanded = self.session.expanded;
        let peeking = self.session.peek.is_some() || expanded;
        let reference = self
            .session
            .target_row()
            .filter(|_| peeking)
            .map(|row| row.reference.clone());
        let logged = query == TaskQuery::RecentActions;
        let settings = self.session.settings.is_some();
        let width = window.viewport_size().width;
        let hints = width > px(HEADER_HINTS_MIN_WINDOW_WIDTH);
        let crumbs = !settings && width > px(CRUMBS_MIN_WINDOW_WIDTH);
        div()
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
                                    .id("crumb-workspace")
                                    .relative()
                                    .role(Role::Button)
                                    .aria_label("Switch workspace")
                                    .hint(Hint::new("Switch workspace").keys("g w"))
                                    .text_color(t.muted)
                                    .hover(|style| style.text_color(t.ink))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_workspace(&PickWorkspace, window, cx);
                                    }))
                                    .child(anchor_probe("workspace", &self.anchors))
                                    .child(snapshot.workspace.clone()),
                            )
                            .child(div().text_color(t.faint).child("›"))
                            .child(
                                div()
                                    .id("crumb-scope")
                                    .relative()
                                    .role(Role::Button)
                                    .aria_label("Choose project scope")
                                    .hint(Hint::new("Choose project scope").keys("g p"))
                                    .text_color(t.muted)
                                    .hover(|style| style.text_color(t.ink))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_project(&PickProject, window, cx);
                                    }))
                                    .child(anchor_probe("scope", &self.anchors))
                                    .child(scope_name),
                            )
                            .child(div().text_color(t.faint).child("›"))
                    })
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child(name),
                    )
                    .when_some(reference, |crumb, reference| {
                        crumb
                            .child(div().text_color(t.faint).child("›"))
                            .child(div().text_color(t.muted).child(reference))
                    }),
            )
            .when(hints && !expanded && !settings, |top| {
                let (actions, tasks) = snapshot.recent.today;
                let text = if logged {
                    format!(
                        "{actions} {} on {tasks} {} today",
                        if actions == 1 { "action" } else { "actions" },
                        if tasks == 1 { "task" } else { "tasks" },
                    )
                } else {
                    format!("{count} tasks")
                };
                top.child(div().flex_none().text_color(t.faint).child(text))
            })
            .child(
                small(Text::Secondary)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.faint)
                    .when(hints && !expanded && !settings, |hint| hint.child(subtitle)),
            )
            .map(|top| self.render_top_controls(top, cx, &t))
            .child(
                btn("search", "Search", &t)
                    .shadow(vec![t.inset_ring(t.line2)])
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_search(&OpenSearch, window, cx)
                        }),
                    )
                    .child("Search")
                    .map(|search| {
                        if hints {
                            search.child(small(Text::Secondary).text_color(t.faint).child("/"))
                        } else {
                            search.hint(Hint::new("Search").keys("/"))
                        }
                    }),
            )
            .child(
                btn("palette", "Commands", &t)
                    .shadow(vec![t.inset_ring(t.line2)])
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_palette(&OpenPalette, window, cx)
                    }))
                    .hint(Hint::new("Command palette").keys(":"))
                    .child(by_platform!("⌘K", "Ctrl+K")),
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
                    .when(!hints, |button| {
                        button.hint(Hint::new("New task").keys("a"))
                    })
                    .when(hints, |button| {
                        button.child(
                            small(Text::Secondary)
                                .opacity(0.7)
                                .font_weight(FontWeight::NORMAL)
                                .child("a"),
                        )
                    }),
            )
    }

    fn render_top_controls(&self, top: Div, cx: &mut Context<Self>, t: &Theme) -> Div {
        let view = &self.session.snapshot.view;
        let query = view.query;
        let expanded = self.session.expanded;
        let name = if self.session.settings.is_some() {
            "Settings"
        } else if self.session.review.is_some() {
            "Triage"
        } else {
            view_name(query)
        };
        let fixed = matches!(query, TaskQuery::Queue | TaskQuery::Upcoming);
        let logged = query == TaskQuery::RecentActions;
        let settings = self.session.settings.is_some();
        let filterable = !settings && !logged && query != TaskQuery::Conflicts;
        let can_columns = !settings && supports_columns(query) && self.session.review.is_none();
        let columns = view.columns;
        let reviewing = self.session.review.is_some();
        let orderable = !settings
            && !matches!(query, TaskQuery::Upcoming | TaskQuery::Overdue)
            && !logged
            && !reviewing;
        let groupable = !settings
            && !fixed
            && !logged
            && !reviewing
            && !columns
            && !matches!(query, TaskQuery::Epics | TaskQuery::Conflicts);
        let dated = !settings
            && !logged
            && !reviewing
            && !columns
            && !matches!(query, TaskQuery::Epics | TaskQuery::Recurring);
        top.when(
            matches!(query, TaskQuery::Inbox | TaskQuery::Queue)
                && !expanded
                && !reviewing
                && !settings,
            |top| {
                top.child(
                    line_btn("review-tasks", "Review tasks", t)
                        .shadow(Vec::new())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.review_tasks(&ReviewTasks, window, cx);
                        })),
                )
            },
        )
        .when(query == TaskQuery::Epics && !expanded && !settings, |top| {
            let closed = if view.closed {
                "Hide closed"
            } else {
                "Show closed"
            };
            top.child(
                btn("epics-closed", closed, t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_closed(window, cx);
                    }))
                    .child(closed),
            )
            .child(
                btn("new-epic", "New epic", t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_epic(&NewEpic, window, cx);
                    }))
                    .child("New epic"),
            )
        })
        .when(query == TaskQuery::Recurring && !settings, |top| {
            top.child(
                btn("new-recurring", "New recurring task", t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_recurring(window, cx);
                    }))
                    .child("New recurring task"),
            )
        })
        .when(expanded, |top| {
            top.child(
                line_btn("collapse", format!("← {name}"), t)
                    .hint(Hint::new("Back to the list").keys("esc"))
                    .on_click(cx.listener(|this, _, window, cx| this.collapse(window, cx))),
            )
        })
        .when(!expanded && !reviewing && filterable, |top| {
            top.child(
                btn("filter", "Filter", t)
                    .relative()
                    .child(anchor_probe("filter", &self.anchors))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.pick_filter(&PickFilter, window, cx);
                    }))
                    .child("Filter"),
            )
        })
        .when(!expanded && orderable, |top| {
            top.child(
                btn("order", "Order", t)
                    .hint(Hint::new("Change order").keys("o"))
                    .relative()
                    .child(anchor_probe("order", &self.anchors))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.pick_order(&PickOrder, window, cx);
                    }))
                    .child(crate::commands::order_label(view)),
            )
        })
        .when(!expanded && groupable, |top| {
            top.child(
                btn("group", "Group", t)
                    .relative()
                    .child(anchor_probe("group", &self.anchors))
                    .on_click(cx.listener(|this, _, window, cx| this.pick_group(window, cx)))
                    .child("Group"),
            )
        })
        .when(!expanded && can_columns, |top| {
            let label = if columns { "List" } else { "Columns" };
            top.child(
                btn("layout", label, t)
                    .hint(
                        Hint::new(if columns {
                            "Show as a list"
                        } else {
                            "Show as columns"
                        })
                        .keys("v l"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_layout(&ToggleLayout, window, cx);
                    }))
                    .child(label),
            )
        })
        .when(
            !expanded && !settings && query != TaskQuery::RecentActions,
            |top| {
                top.child(
                    btn("details", "Toggle details", t)
                        .hint(
                            Hint::new(if self.details_open {
                                "Hide details"
                            } else {
                                "Show details"
                            })
                            .keys("g d"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_details(&ToggleDetails, window, cx)
                        }))
                        .child(icon("icons/ic-details.svg", t.body))
                        .child("Details"),
                )
            },
        )
        .when(!expanded && dated, |top| {
            top.child(
                btn("display", "Display", t)
                    .relative()
                    .aria_expanded(self.display_open())
                    .child(anchor_probe("display", &self.anchors))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_display(window, cx);
                    }))
                    .child("Display"),
            )
        })
    }

    /// The lines between the header and the list: what scopes the view.
    fn render_strips(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let snapshot = &self.session.snapshot;
        let view = &snapshot.view;
        let mut strips = Vec::new();
        if !self.session.show_task_strips() {
            return strips;
        }
        let strip = || {
            div()
                .flex_none()
                .min_h(px(34.))
                .px(px(16.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .border_b_1()
                .border_color(t.line)
                .text(Text::Secondary)
                .text_color(t.muted)
        };
        if let Some(text) = &view.search {
            let count = snapshot.ordered().count();
            let scope = view
                .project
                .as_ref()
                .and_then(|key| snapshot.projects.iter().find(|project| project.key == *key))
                .map_or("all projects".to_string(), |project| project.name.clone());
            strips.push(
                strip()
                    .child(
                        div()
                            .flex_1()
                            .child(format!("{count} results for “{text}” in {scope}")),
                    )
                    .child(
                        btn("search-refine", "Refine", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_search(&OpenSearch, window, cx);
                            }))
                            .child("Refine"),
                    )
                    .child(
                        btn("search-clear", "Clear", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_view(&ShowView(TaskQuery::Queue), window, cx);
                            }))
                            .child("Clear"),
                    )
                    .into_any_element(),
            );
        }
        if view.project.is_some() {
            let count = |key: &str| {
                let counts = snapshot.views.iter();
                counts
                    .clone()
                    .find(|(view, _)| *view == key)
                    .map_or(0, |(_, n)| *n)
            };
            let parts = [
                ("active", "active"),
                ("ready", "ready"),
                ("blocked", "blocked"),
                ("upcoming", "deferred"),
            ];
            let text: Vec<String> = parts
                .iter()
                .map(|(key, word)| format!("{} {word}", count(key)))
                .collect();
            strips.push(strip().child(text.join("  ·  ")).into_any_element());
        }
        if view.has_filters() {
            let mut chips: Vec<(String, Command)> = Vec::new();
            if let Some(label) = &view.label {
                chips.push((format!("Label: {label}"), Command::FilterLabel(None)));
            }
            if let Some(priority) = view.priority {
                let name = priority_name(priority);
                chips.push((format!("Priority: {name}"), Command::FilterPriority(None)));
            }
            match view.deleted {
                DeletedFilter::Hide => {}
                DeletedFilter::Show => chips.push((
                    "Including deleted".to_string(),
                    Command::FilterDeleted(DeletedFilter::Hide),
                )),
                DeletedFilter::Only => chips.push((
                    "Deleted only".to_string(),
                    Command::FilterDeleted(DeletedFilter::Hide),
                )),
            }
            if let Some(filter) = &view.metadata {
                let word = if filter.present { "present" } else { "missing" };
                chips.push((
                    format!("{} {word}", filter.key),
                    Command::FilterMetadata(None),
                ));
            }
            let mut line = strip();
            for (ix, (label, command)) in chips.into_iter().enumerate() {
                line = line.child(
                    bare_btn(("filter-chip", ix), format!("Remove {label}"), &t)
                        .h(px(22.))
                        .px(px(8.))
                        .rounded_full()
                        .text(Text::Secondary)
                        .shadow(vec![t.inset_ring(t.line2)])
                        .hover(|style| style.bg(t.raised))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.session.write(command.clone());
                            cx.notify();
                        }))
                        .child(label)
                        .child(div().text_color(t.faint).child("×")),
                );
            }
            strips.push(
                line.child(
                    btn("filters-clear", "Clear filters", &t)
                        .h(px(22.))
                        .text(Text::Secondary)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.clear_filters(&ClearFilters, window, cx);
                        }))
                        .child("Clear filters"),
                )
                .into_any_element(),
            );
        }
        strips
    }

    /// What can follow the first key of a chord.
    fn render_chord(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let key = self.chord.as_ref()?;
        let (_, keys) = CHORDS.iter().find(|(first, _)| *first == key)?;
        let t = theme(cx);
        let mut bar = div()
            .id("chord")
            .role(Role::Status)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(RADIUS_SURFACE))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .text(Text::Secondary)
            .text_color(t.body)
            .child(div().font_family(MONO).text_color(t.ink).child(key.clone()));
        for (next, name) in keys.iter() {
            bar = bar.child(
                div()
                    .flex()
                    .gap(px(5.))
                    .child(div().font_family(MONO).text_color(t.accent).child(*next))
                    .child(*name),
            );
        }
        Some(
            div()
                .absolute()
                .bottom(px(66.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(bar),
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
        let (glide_bg, glide_ring) = t.selection(window.is_window_active());
        let glide = self.glide.filter(|glide| glide.live()).map(|glide| {
            let scrolled = f32::from(offset);
            div().absolute().inset_0().overflow_hidden().child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .h(px(row_height()))
                    .rounded(px(RADIUS_CONTROL))
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
        let left_rows = self.list_motion.left_rows();
        let left_rows = left_rows.into_iter().map(|(row, run)| {
            div().absolute().inset_0().overflow_hidden().child(
                div()
                    .absolute()
                    .top(px(row.top))
                    .left(px(8.))
                    .right(px(8.))
                    .h(px(row_height()))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .bg(t.page)
                    .text_color(t.body)
                    .child(ref_label(&row.reference, &row.project_key, None, &t))
                    .child(div().min_w_0().truncate().child(row.title))
                    .with_animation(
                        ("row-leave", run),
                        Animation::new(LEAVE_DURATION).with_easing(ease_in_out),
                        |row, delta| {
                            row.h(px(row_height() * (1. - delta)))
                                .opacity(1. - (delta / 0.7).min(1.))
                        },
                    ),
            )
        });
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .children(glide)
            .children(left)
            .children(left_rows)
            .child(ListBounce::new(
                "list-bounce",
                self.bounce.clone(),
                self.list.clone(),
                list(self.list.clone(), cx.processor(Self::render_entry)).size_full(),
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

/// The box a marked row carries, and the gap an unmarked one keeps.
/// A row's date in figures that line up, naming the full time on hover.
fn date_cell(id: (&'static str, usize), name: &str, date: RowDate, t: &Theme) -> Stateful<Div> {
    let full = format!("{name} {}", date.full);
    small(Text::Secondary)
        .id(id)
        .flex_none()
        .min_w(px(44.))
        .text_right()
        .whitespace_nowrap()
        .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
        .text_color(t.faint)
        .aria_label(full.clone())
        .hint(Hint::new(full))
        .child(date.day)
}

fn mark_box(marked: bool, t: &Theme) -> Div {
    if !marked {
        return div().flex_none().w(px(3.)).h(px(16.)).ml(px(-5.));
    }
    div()
        .flex_none()
        .size(px(14.))
        .ml(px(-2.))
        .rounded(px(RADIUS_CHIP))
        .bg(t.accent)
        .flex()
        .items_center()
        .justify_center()
        .text(Text::Caption)
        .text_color(t.on_fill)
        .child("✓")
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

/// Reveals the selected row with breathing room and its immediately preceding
/// section heading. In a short viewport, the row takes priority over the heading.
fn selection_scroll_offset(items: &[Item], selected: usize, current: f32, viewport: f32) -> f32 {
    const MARGIN: f32 = 4.;
    let row_top: f32 = items[..selected].iter().map(Item::height).sum();
    let row_bottom = row_top + items[selected].height();
    let mut top = row_top;
    if selected > 0 && matches!(items[selected - 1], Item::Section { .. }) {
        top -= items[selected - 1].height();
    }
    top = (top - MARGIN).max(0.);
    let bottom = row_bottom + MARGIN;
    if bottom - top > viewport {
        top = (row_top - MARGIN).max(0.);
    }
    let target = if top < current {
        top
    } else if bottom > current + viewport {
        (bottom - viewport).min(top)
    } else {
        current
    };
    let total: f32 = items.iter().map(Item::height).sum();
    target.clamp(0., (total - viewport).max(0.))
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

impl Root {
    fn root_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        let reviewing = self.session.review.is_some();
        let recurring = self.session.snapshot.view.query == TaskQuery::Recurring;
        // A menu item with no handler here greys out, as it would do nothing. Popup
        // menus do not count: palette entries dispatch from a frame that shows one.
        let unblocked = !self.composer_open(cx) && !self.dialog_open();
        let toast_action = self
            .session
            .toast()
            .is_some_and(|toast| toast.action.is_some());
        let settings_closed = self.session.settings.is_none();
        let on_list = unblocked && settings_closed && !reviewing;
        let has_task = self.session.target().is_some() || recurring;
        let logged = self.session.snapshot.view.query == TaskQuery::RecentActions;
        root.when(unblocked, |el| {
            el.on_action(cx.listener(Self::open_settings))
                .on_action(cx.listener(Self::open_palette))
                .on_action(cx.listener(Self::open_help))
                .on_action(cx.listener(Self::new_task))
                .on_action(cx.listener(Self::open_about))
        })
        .when(on_list, |el| {
            el.on_action(cx.listener(Self::move_down))
                .on_action(cx.listener(Self::move_up))
                .on_action(cx.listener(Self::lane_back))
                .on_action(cx.listener(Self::lane_on))
                .on_action(cx.listener(Self::all_projects))
                .on_action(cx.listener(Self::pick_project))
                .on_action(cx.listener(Self::pick_workspace))
                .on_action(cx.listener(Self::toggle_layout))
                .on_action(cx.listener(Self::open_search))
                .on_action(cx.listener(Self::pick_filter))
                .on_action(cx.listener(Self::pick_priority_filter))
                .on_action(cx.listener(Self::pick_label_filter))
                .on_action(cx.listener(Self::pick_metadata_filter))
                .on_action(cx.listener(Self::cycle_deleted))
                .on_action(cx.listener(Self::clear_filters))
                .on_action(cx.listener(Self::pick_order))
                .on_action(cx.listener(Self::show_view))
                .on_action(cx.listener(Self::toggle_mark))
                .on_action(cx.listener(Self::extend_marks_down))
                .on_action(cx.listener(Self::extend_marks_up))
                .on_action(cx.listener(Self::mark_visible))
                .on_action(cx.listener(Self::clear_marks))
                .on_action(cx.listener(Self::focus_child))
                .on_action(cx.listener(Self::new_epic))
                .on_action(cx.listener(Self::cycle_lifecycle))
                .on_action(cx.listener(Self::review_tasks))
        })
        .when(on_list && !has_task && logged, |el| {
            el.on_action(cx.listener(Self::expand))
        })
        .when(on_list && has_task, |el| {
            el.on_action(cx.listener(Self::mark_done))
                .on_action(cx.listener(Self::set_status))
                .on_action(cx.listener(Self::cancel_task))
                .on_action(cx.listener(Self::change_status))
                .on_action(cx.listener(Self::edit_title))
                .on_action(cx.listener(Self::rename_task))
                .on_action(cx.listener(Self::edit_description))
                .on_action(cx.listener(Self::edit_due))
                .on_action(cx.listener(Self::change_project))
                .on_action(cx.listener(Self::edit_labels))
                .on_action(cx.listener(Self::edit_metadata))
                .on_action(cx.listener(Self::focus_note))
                .on_action(cx.listener(Self::delete_task))
                .on_action(cx.listener(Self::restore_task))
                .on_action(cx.listener(Self::add_blocker))
                .on_action(cx.listener(Self::remove_blocker))
                .on_action(cx.listener(Self::add_related))
                .on_action(cx.listener(Self::jump_to_blocker))
                .on_action(cx.listener(Self::jump_to_epic))
                .on_action(cx.listener(Self::expand))
                .on_action(cx.listener(Self::copy_ref))
                .on_action(cx.listener(Self::copy_id))
                .on_action(cx.listener(Self::copy_title))
                .on_action(cx.listener(Self::copy_description))
                .on_action(cx.listener(Self::copy_title_and_description))
                .on_action(cx.listener(Self::copy_notes))
                .on_action(cx.listener(Self::copy_report))
                .on_action(cx.listener(Self::add_to_epic))
                .on_action(cx.listener(Self::publish_gist))
        })
        .when(unblocked && settings_closed && has_task, |el| {
            el.on_action(cx.listener(Self::change_priority))
                .on_action(cx.listener(Self::edit_availability))
        })
        .on_action(cx.listener(Self::minimize))
        .on_action(cx.listener(Self::zoom))
        .on_action(cx.listener(Self::toggle_full_screen))
        .on_action(cx.listener(Self::toggle_theme))
        .on_action(cx.listener(Self::close_settings))
        .on_action(cx.listener(Self::open_sync))
        .on_action(cx.listener(Self::sync_now))
        .on_action(cx.listener(Self::toggle_details))
        .on_action(cx.listener(Self::toggle_sidebar))
        .on_action(cx.listener(Self::save_description))
        .on_action(cx.listener(Self::back))
        .on_action(cx.listener(Self::undo))
        .when(toast_action, |el| {
            el.on_action(cx.listener(Self::run_toast_action))
        })
        .on_action(cx.listener(Self::paste_image))
        .on_action(cx.listener(Self::dismiss))
        .on_action(cx.listener(Self::next_field))
        .on_action(cx.listener(Self::previous_field))
        .on_action(cx.listener(Self::close_window))
        .on_action(cx.listener(Self::review_decide))
        .on_action(cx.listener(Self::review_keep))
        .on_action(cx.listener(Self::review_skip))
        .on_action(cx.listener(Self::review_back))
        .on_action(cx.listener(Self::review_undo))
        .on_action(cx.listener(Self::review_exit))
        .on_action(cx.listener(Self::review_next))
    }

    fn render_sync_state(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let t = theme(cx);
        let (tone, label) = self.session.sync_line();
        let color = sync_icon(&t, tone);
        let problem = matches!(tone, SyncTone::Warn | SyncTone::Bad);
        let raised = t.raised;
        div()
            .id("sync-state")
            .role(Role::Button)
            .aria_label(label.clone())
            .hint(
                Hint::new(label.clone())
                    .sub("Sync now")
                    .sub_keys(by_platform!("⇧S", "Shift+S")),
            )
            .min_w_0()
            .h(px(28.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(RADIUS_CONTROL))
            .text_color(color)
            .hover(move |style| style.bg(raised))
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_sync(&OpenSync, window, cx);
            }))
            .when_some(self.session.motion.pulse(), |state, (changes, run)| {
                state.child(sync_pulse(changes, run, &t))
            })
            .child(icon("icons/cloud.svg", color))
            .when(problem, |state| {
                state.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
            })
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
        if FIRST.swap(false, std::sync::atomic::Ordering::Relaxed) && crate::startup::enabled() {
            crate::startup::mark("first-render");
            window.on_next_frame(|_, _| crate::startup::mark("first-frame-ready"));
        }
        self.arm_toast(cx);
        self.focus_sync_page(window, cx);
        self.focus_update_phase(window, cx);
        self.sync_welcome(window, cx);
        if self.welcome.is_some() {
            return self.render_welcome(window, cx);
        }
        let t = theme(cx);
        let narrow = window.viewport_size().width <= px(DETAIL_MIN_WINDOW_WIDTH);
        if self.narrow != Some(narrow) {
            self.details_open = !narrow && (self.narrow.is_some() || self.details_open);
            self.narrow = Some(narrow);
        }
        let mut key_context = KeyContext::default();
        key_context.add("Aven");
        if self.session.settings.is_some() {
            key_context.add("Settings");
        }
        if self.session.review.is_some() {
            key_context.add("Review");
        }
        if self.menu.is_some()
            || self.display_open()
            || self.composer_open(cx)
            || self.dialog_open()
        {
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
        self.list_dues = self
            .session
            .snapshot
            .tasks
            .iter()
            .any(|task| !task.due.is_empty());
        let expanded = self.session.expanded;
        let columns = self.session.snapshot.view.columns;
        let list_el = self.render_list(window, cx);
        let board =
            (columns && !expanded).then(|| self.render_board(window.is_window_active(), cx));
        let doc = expanded.then(|| self.render_doc(window, cx));
        let logged = self.session.snapshot.view.query == TaskQuery::RecentActions;
        let reviewing = self.session.review.is_some();
        let details_open = self.details_open && !expanded && !logged && !reviewing;
        let recurring = self.session.snapshot.view.query == TaskQuery::Recurring;
        let bulk = self.session.bulk_panel();
        let detail = details_open.then(|| {
            if recurring {
                self.render_series(detail_width, cx).into_any_element()
            } else if bulk {
                self.render_bulk(detail_width, cx).into_any_element()
            } else {
                self.render_detail(detail_width, window, cx)
                    .into_any_element()
            }
        });
        let review = reviewing.then(|| self.render_review(cx));
        let settings = self
            .session
            .settings
            .is_some()
            .then(|| self.render_settings(window, cx));
        let strips = self.render_strips(cx);
        let chord = self.render_chord(cx);
        let dialog = self.render_dialog(window, cx);
        let menu = self.render_menu(window, cx);
        let display = self.render_display(window, cx);
        let composer = self.render_composer(window, cx);
        let toast = self.render_toast(cx);
        let nav_edge = self.prefs.nav_width;
        let split = self.split_drag;
        let nav_open = self.nav_open;
        let nav_handle = self.split_handle(Split::Nav, cx);
        let detail_handle = self.split_handle(Split::Detail, cx);
        if self.session.database_error.is_some() {
            let gate = self.render_database_gate(cx);
            let dialog = self.render_dialog(window, cx);
            let toast = self.render_toast(cx);
            return div()
                .relative()
                .size_full()
                .flex()
                .bg(t.page)
                .text_color(t.ink)
                .font_family(".SystemUIFont")
                .text(Text::Body)
                .line_height(px(UI_LINE_HEIGHT))
                .track_focus(&self.focus)
                .on_action(cx.listener(Self::next_field))
                .on_action(cx.listener(Self::previous_field))
                .on_action(cx.listener(Self::dismiss))
                .on_action(cx.listener(Self::close_window))
                .on_action(cx.listener(Self::minimize))
                .on_action(cx.listener(Self::zoom))
                .on_action(cx.listener(Self::toggle_full_screen))
                .on_action(cx.listener(Self::run_toast_action))
                .child(gate)
                .children(dialog)
                .children(toast);
        }
        div()
            .relative()
            .size_full()
            .flex()
            .bg(t.chrome)
            .text_color(t.ink)
            .font_family(".SystemUIFont")
            .text(Text::Body)
            .line_height(px(UI_LINE_HEIGHT))
            .key_context(key_context)
            .track_focus(&self.focus)
            .map(|root| self.root_actions(root, cx))
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
                div()
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
                    .rounded(px(RADIUS_SURFACE))
                    .bg(t.page)
                    .shadow(t.sheet_shadow())
                    .overflow_hidden()
                    .child(top)
                    .children(strips)
                    .child(div().flex_1().min_h_0().flex().map(|body| {
                        match settings {
                            Some(settings) => body.child(settings),
                            None => body
                                .when_some(doc, |body, doc| body.child(doc))
                                .when_some(review, |body, review| body.child(review))
                                .when(!expanded && !reviewing, |body| match board {
                                    Some(board) => body.child(board),
                                    None => body.child(list_el),
                                })
                                .when_some(detail, |body, detail| {
                                    body.child(div().flex().flex_none().h_full().child(detail))
                                }),
                        }
                    })),
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
            .when_some(chord, |root, chord| root.child(chord))
            .when_some(composer, |root, composer| root.child(composer))
            .when_some(dialog, |root, dialog| root.child(dialog))
            .when_some(menu, |root, menu| root.child(menu))
            .when_some(display, |root, display| root.child(display))
            .when_some(toast, |root, toast| root.child(toast))
            .map(|root| crate::tooltip::host(root, window, cx, &t))
    }
}

/// A change arrived: a ring leaves the status dot and the count fades through.
fn sync_pulse(changes: usize, run: usize, t: &Theme) -> impl IntoElement + use<> {
    let good = t.good;
    let count = count_text(changes, "change");
    div()
        .flex()
        .items_center()
        .gap(px(11.))
        .text_color(t.faint)
        .whitespace_nowrap()
        .child(
            div()
                .flex_none()
                .size(px(7.))
                .rounded_full()
                .bg(good)
                .relative()
                .with_animation(
                    ("sync-pulse-ring", run),
                    Animation::new(PULSE_DURATION),
                    move |dot, delta| {
                        let spread = 1. - (1. - (delta / 0.65).min(1.)).powi(3);
                        let ring = 7. + 15. * spread;
                        dot.when(delta < 0.65, |dot| {
                            dot.child(
                                div()
                                    .absolute()
                                    .top(px(3.5 - ring / 2.))
                                    .left(px(3.5 - ring / 2.))
                                    .size(px(ring))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(good.opacity((1. - spread) * 0.6)),
                            )
                        })
                    },
                ),
        )
        .child(div().child(count).with_animation(
            ("sync-pulse-count", run),
            Animation::new(PULSE_DURATION),
            |count, delta| count.opacity(1. - ((delta - 0.75) / 0.25).clamp(0., 1.)),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(name: &str) -> Item {
        Item::Section {
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
    fn keyboard_reveal_keeps_headers_and_row_edges_visible() {
        let id = TaskId::new();
        let items = [
            Item::Pad(4.),
            section("Active"),
            row(&id),
            row(&id),
            section("Todo"),
            row(&id),
            row(&id),
            Item::Pad(40.),
        ];
        let height = row_height();
        assert_eq!(selection_scroll_offset(&items, 2, 36., 100.), 0.);
        // The first row of a middle section brings its heading back too.
        let heading_top = 36. + 2. * height;
        assert_eq!(
            selection_scroll_offset(&items, 5, heading_top + 32., 100.),
            heading_top - 4.,
        );
        // Moving down leaves room below the last row, not just its border.
        let last_bottom = heading_top + 32. + 2. * height;
        assert_eq!(
            selection_scroll_offset(&items, 6, 0., 100.),
            last_bottom + 4. - 100.,
        );
        // An already visible row doesn't move the viewport.
        assert_eq!(selection_scroll_offset(&items, 3, 20., 100.), 20.);
        // A viewport too short for a heading still shows the complete row.
        let top = selection_scroll_offset(&items, 5, 0., height + 8.);
        assert_eq!(top, heading_top + 32. - 4.);
        assert_eq!(selection_scroll_offset(&items, 2, 36., 1000.), 0.);
    }

    #[test]
    fn keyboard_reveal_preserves_top_padding_without_sections() {
        let id = TaskId::new();
        let items = [Item::Pad(4.), row(&id), row(&id), Item::Pad(40.)];
        assert_eq!(selection_scroll_offset(&items, 1, 4., 50.), 0.);
        let bottom = 4. + 2. * row_height();
        assert_eq!(
            selection_scroll_offset(&items, 2, 0., 50.),
            bottom + 4. - 50.
        );
    }

    #[test]
    fn a_scroll_position_lands_in_the_entry_that_holds_it() {
        let id = TaskId::new();
        let items = [Item::Pad(4.), section("Focus"), row(&id)];
        let at = list_offset_at(&items, 40.);
        assert_eq!((at.item_ix, at.offset_in_item), (2, px(4.)));
    }
}

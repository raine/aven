//! The app's state above the store: selection, drafts, request identity, messages
//! and motion bookkeeping. It has no GPUI types, so behaviour tests need no window.
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::ids::TaskId;
use aven_core::recurrence::RecurrenceSeriesId;
use aven_host::store::TaskQuery;
use tokio::sync::mpsc::UnboundedSender;

use crate::adapter::{
    Bulk, CatalogTask, Command, Detail, Go, NewTask, Reply, ReplyBody, Request, ReviewCard,
    SearchInput, SearchResults, SeriesDetail, Snapshot, SyncHealth, TaskRow, done_group,
};

pub const POP_DURATION: Duration = Duration::from_millis(320);
pub const SETTLE_DURATION: Duration = Duration::from_millis(620);
pub const PRIORITY_DURATION: Duration = Duration::from_millis(200);
pub const COPIED_DURATION: Duration = Duration::from_millis(1300);
pub const RING_DURATION: Duration = Duration::from_millis(700);
pub const ALL_CLEAR_DURATION: Duration = Duration::from_millis(1000);
const REVIEW_REPEAT_WINDOW: Duration = Duration::from_millis(500);

/// Motions keyed on wall time. Each expires on its own, so a row scrolled back
/// into view does not replay it, and a refresh never starts one.
#[derive(Debug, Default)]
pub struct Motion {
    reduced: bool,
    runs: usize,
    completed: HashMap<TaskId, (TaskStatus, usize, Instant)>,
    repriced: HashMap<TaskId, (TaskPriority, usize, Instant)>,
    copied: Option<(TaskId, usize, Instant)>,
    /// Epics whose ring sweeps on from an earlier count.
    rings: HashMap<TaskId, (usize, usize, Instant)>,
    all_clear: Option<(usize, Instant)>,
}

impl Motion {
    pub fn set_reduced(&mut self, reduced: bool) {
        self.reduced = reduced;
    }

    pub fn reduced(&self) -> bool {
        self.reduced
    }

    /// Numbers an animation, so each one starts over.
    pub fn next_run(&mut self) -> usize {
        self.runs += 1;
        self.runs
    }

    fn status_changed(&mut self, id: &TaskId, previous: TaskStatus, status: TaskStatus) {
        self.completed
            .retain(|_, (_, _, at)| at.elapsed() < SETTLE_DURATION);
        if status == TaskStatus::Done && previous != TaskStatus::Done && !self.reduced {
            let run = self.next_run();
            self.completed
                .insert(id.clone(), (previous, run, Instant::now()));
        } else {
            self.completed.remove(id);
        }
    }

    fn priority_changed(&mut self, id: &TaskId, previous: TaskPriority, priority: TaskPriority) {
        self.repriced
            .retain(|_, (_, _, at)| at.elapsed() < PRIORITY_DURATION);
        if previous != priority && !self.reduced {
            let run = self.next_run();
            self.repriced
                .insert(id.clone(), (previous, run, Instant::now()));
        }
    }

    fn ring_changed(&mut self, id: &TaskId, from: usize) {
        if !self.reduced {
            let run = self.next_run();
            self.rings.insert(id.clone(), (from, run, Instant::now()));
        }
    }

    /// Whether a ring or All clear needs frames until it ends.
    pub fn ringing(&self) -> bool {
        self.all_clear().is_some()
            || (self.rings.values()).any(|(_, _, at)| at.elapsed() < RING_DURATION)
    }

    /// The count a live ring started from, and its animation.
    pub fn ring(&self, id: &TaskId) -> Option<(usize, usize)> {
        let (from, run, at) = self.rings.get(id)?;
        (at.elapsed() < RING_DURATION).then_some((*from, *run))
    }

    fn cleared(&mut self) {
        if !self.reduced {
            self.all_clear = Some((self.next_run(), Instant::now()));
        }
    }

    pub fn all_clear(&self) -> Option<usize> {
        let (run, at) = self.all_clear?;
        (at.elapsed() < ALL_CLEAR_DURATION).then_some(run)
    }

    pub fn copied(&mut self, id: &TaskId) {
        let run = self.next_run();
        self.copied = Some((id.clone(), run, Instant::now()));
    }

    /// What a just-completed task was, and its run.
    pub fn completion(&self, id: &TaskId) -> Option<(TaskStatus, usize)> {
        let (previous, run, at) = self.completed.get(id)?;
        (at.elapsed() < SETTLE_DURATION).then_some((*previous, *run))
    }

    pub fn priority_change(&self, id: &TaskId) -> Option<(TaskPriority, usize)> {
        let (previous, run, at) = self.repriced.get(id)?;
        (at.elapsed() < PRIORITY_DURATION).then_some((*previous, *run))
    }

    /// The run of a copy still showing its feedback.
    pub fn copy(&self, id: &TaskId) -> Option<usize> {
        let (copied, run, at) = self.copied.as_ref()?;
        (copied == id && at.elapsed() < COPIED_DURATION).then_some(*run)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastAction {
    /// Runs a command that did not save.
    Retry(Command),
    /// Reloads after a save whose refresh failed. It never repeats the write.
    Refresh,
    /// Moves somewhere the write made interesting.
    Go(Go),
    /// Brings a discarded draft back, which the window holds.
    RestoreDraft,
}

impl ToastAction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Retry(_) => "Retry",
            Self::Refresh => "Refresh",
            Self::Go(go) => go.label,
            Self::RestoreDraft => "Restore",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub run: usize,
    pub message: String,
    pub action: Option<ToastAction>,
    pub undo: bool,
}

/// What a reply changed, for the view.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub list: bool,
    pub selection: bool,
    /// A failed note went back into the selected task's draft.
    pub draft: bool,
    /// A task that could not be created.
    pub composer_draft: Option<Box<NewTask>>,
    /// A draft natural language input produced.
    pub intake: Option<Box<NewTask>>,
    pub intake_failed: bool,
    /// Text for the clipboard, with what to call it.
    pub copy: Option<String>,
    /// The task whose local write produced this list, so the list may animate.
    pub moved: Option<TaskId>,
    /// A series whose replacement exists, to ask about stopping.
    pub stop: Option<RecurrenceSeriesId>,
    /// A series that became the selected one.
    pub series: bool,
    /// Rows a local write took off the list.
    pub left: Vec<TaskRow>,
}

/// A change a review made, with the card it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewChange {
    Store(usize),
    Keep(usize),
}

/// A walk through tasks that need a decision, one card at a time.
#[derive(Debug)]
pub struct Review {
    pub cards: Vec<ReviewCard>,
    /// The card on screen. Past the last one, the review shows its summary.
    pub at: usize,
    /// How many tasks wait beyond this page.
    pub more: usize,
    pub decided: BTreeSet<TaskId>,
    /// Tasks every page so far showed, which the next page leaves out.
    seen: Vec<TaskId>,
    changes: Vec<ReviewChange>,
    last_key: Option<(char, Instant)>,
    /// Marks to hand back, since a review only writes to its card.
    marks: BTreeSet<TaskId>,
    /// The cards passed and decided across the review.
    total_decided: usize,
}

impl Review {
    pub fn card(&self) -> Option<&ReviewCard> {
        self.cards.get(self.at)
    }

    /// A key held down repeats faster than anyone reads the next card.
    fn repeated(&mut self, key: char) -> bool {
        let now = Instant::now();
        let repeat = self
            .last_key
            .is_some_and(|(last, at)| last == key && now - at < REVIEW_REPEAT_WINDOW);
        self.last_key = Some((key, now));
        repeat
    }

    /// Moves on to the next undecided card, or to the summary.
    fn advance(&mut self) {
        let next =
            (self.at + 1..self.cards.len()).find(|at| !self.decided.contains(&self.cards[*at].id));
        self.at = next.unwrap_or(self.cards.len());
    }
}

fn join_drafts(first: &str, second: &str) -> String {
    match (first.is_empty(), second.is_empty()) {
        (_, true) => first.to_string(),
        (true, _) => second.to_string(),
        _ => format!("{first}\n\n{second}"),
    }
}

pub struct Session {
    pub snapshot: Snapshot,
    pub selected: Option<TaskId>,
    /// A task opened from another one, with the tasks it was reached through.
    pub peek: Option<TaskId>,
    pub trail: Vec<Trail>,
    /// The target fills the window.
    pub expanded: bool,
    pub detail: Option<Detail>,
    pub catalog: Vec<CatalogTask>,
    pub search: SearchResults,
    pub search_recent: Vec<String>,
    pub note_drafts: HashMap<TaskId, String>,
    pub toast: Option<Toast>,
    pub sync: Option<SyncHealth>,
    pub motion: Motion,
    /// Tasks every write goes to, instead of the selected one.
    pub marks: BTreeSet<TaskId>,
    /// The series selected in the Recurring view.
    pub series: Option<RecurrenceSeriesId>,
    pub series_detail: Option<SeriesDetail>,
    pub review: Option<Review>,
    /// A child picked in the epic's section, which writes go to.
    pub child: Option<TaskId>,
    /// An epic's finished children are listed.
    pub show_finished: bool,
    gist_running: bool,
    /// Tasks closed during this visit to the view, oldest first. They stay in
    /// the Done group until the user refreshes the view.
    done: Vec<TaskId>,
    /// The newest status or priority write whose list has not arrived.
    moving: Option<(u64, TaskId)>,
    requests: UnboundedSender<Request>,
    next_request: u64,
    last_mutation: u64,
    last_detail: u64,
    /// The selection when the newest create was sent.
    create_from: Option<TaskId>,
    last_search: u64,
}

/// A step back from a task opened from another one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trail {
    pub peek: Option<TaskId>,
    pub label: String,
}

impl Session {
    pub fn new(snapshot: Snapshot, requests: UnboundedSender<Request>) -> Self {
        let selected = first_row(&snapshot);
        let mut this = Self {
            snapshot,
            selected: None,
            peek: None,
            trail: Vec::new(),
            expanded: false,
            detail: None,
            catalog: Vec::new(),
            search: SearchResults::default(),
            search_recent: Vec::new(),
            note_drafts: HashMap::new(),
            toast: None,
            sync: None,
            motion: Motion::default(),
            marks: BTreeSet::new(),
            series: None,
            series_detail: None,
            review: None,
            child: None,
            show_finished: false,
            gist_running: false,
            done: Vec::new(),
            moving: None,
            requests,
            next_request: 0,
            last_mutation: 0,
            last_detail: 0,
            create_from: None,
            last_search: 0,
        };
        if let Some(id) = selected {
            this.select(id);
        }
        this
    }

    fn send(&mut self, command: Command) -> u64 {
        self.next_request += 1;
        let id = self.next_request;
        if command.is_mutation() {
            self.last_mutation = id;
        }
        let _ = self.requests.send(Request { id, command });
        id
    }

    fn load_detail(&mut self, id: TaskId) {
        self.last_detail = self.send(Command::LoadDetail(id));
    }

    pub fn select(&mut self, id: TaskId) {
        if self.selected.as_ref() == Some(&id) && self.peek.is_none() {
            return;
        }
        self.leave_peek();
        self.selected = Some(id.clone());
        self.detail = None;
        self.load_detail(id);
    }

    fn leave_peek(&mut self) {
        self.peek = None;
        self.trail.clear();
    }

    /// The task details and task actions apply to.
    pub fn target(&self) -> Option<&TaskId> {
        self.peek.as_ref().or(self.selected.as_ref())
    }

    /// The target's row, which a peeked task only has through its details.
    pub fn target_row(&self) -> Option<&TaskRow> {
        let id = self.target()?;
        let row = self.snapshot.row(id);
        row.or(self.detail.as_ref().map(|detail| &detail.row))
            .filter(|row| row.id == *id)
    }

    fn any_row(&self, id: &TaskId) -> bool {
        self.snapshot.row(id).is_some()
    }

    /// Opens a linked task, in place if the list shows it.
    pub fn follow(&mut self, id: TaskId) {
        let label = self.target_row().map(|row| row.title.clone());
        self.trail.push(Trail {
            peek: self.peek.clone(),
            label: label.unwrap_or_default(),
        });
        self.peek = Some(id.clone());
        self.detail = None;
        self.last_detail = self.send(Command::Peek(id));
    }

    pub fn trail_back(&mut self) {
        let Some(step) = self.trail.pop() else {
            return;
        };
        self.peek = step.peek;
        self.detail = None;
        if let Some(id) = self.target().cloned() {
            self.last_detail = self.send(Command::Peek(id));
        }
    }

    /// Selects a task the list shows, or follows it there from here.
    pub fn open(&mut self, id: TaskId) {
        if self.any_row(&id) {
            self.select(id);
        } else {
            self.follow(id);
        }
    }

    /// Moves the view to a task and selects it.
    pub fn goto(&mut self, id: TaskId) {
        if self.any_row(&id) {
            self.select(id);
        } else {
            self.switch_view(Command::Goto(id));
        }
    }

    /// Changes what the list shows, which counts as refreshing the view.
    pub fn switch_view(&mut self, command: Command) {
        self.refresh_view();
        self.leave_peek();
        self.expanded = false;
        if !matches!(command, Command::Goto(_)) {
            self.selected = None;
            self.detail = None;
        }
        self.send(command);
    }

    pub fn search_preview(&mut self, input: SearchInput) {
        self.last_search = self.send(Command::Preview(input));
    }

    /// Remembers a search, newest first.
    pub fn search_accepted(&mut self, text: &str) {
        self.search_recent.retain(|recent| recent != text);
        self.search_recent.insert(0, text.to_string());
        self.search_recent.truncate(5);
    }

    /// Tasks a write goes to instead of the selection, in display order. A task
    /// opened on its own takes writes for itself.
    pub fn bulk_targets(&self) -> Option<Vec<TaskId>> {
        if self.marks.is_empty()
            || self.peek.is_some()
            || self.expanded
            || self.child.is_some()
            || self.review.is_some()
        {
            return None;
        }
        Some(
            order(&self.snapshot)
                .into_iter()
                .filter(|id| self.marks.contains(id))
                .collect(),
        )
    }

    /// Whether the details panel speaks for the marks.
    pub fn bulk_panel(&self) -> bool {
        self.bulk_targets()
            .is_some_and(|ids| ids.len() > 1 || ids.first() != self.selected.as_ref())
    }

    /// Writes to the marked tasks, naming them so that a list that changed on
    /// the way cannot redirect the write.
    pub fn bulk(&mut self, bulk: Bulk) -> bool {
        let Some(ids) = self.bulk_targets() else {
            return false;
        };
        let first = ids[0].clone();
        let request = self.send(Command::Bulk(ids, bulk));
        self.moving = Some((request, first));
        true
    }

    pub fn toggle_mark(&mut self, id: TaskId) {
        if !self.marks.remove(&id) {
            self.marks.insert(id);
        }
    }

    /// Marks the selection, which has to be a row.
    pub fn toggle_selected_mark(&mut self) {
        if self.snapshot.view.query == TaskQuery::Recurring {
            self.show_toast(
                "recurring rows select series and cannot be marked. press enter to open an occurrence."
                    .to_string(),
                None,
            );
            return;
        }
        match self.selected.clone() {
            Some(id) => self.toggle_mark(id),
            None => self.show_toast("no selected task to mark".to_string(), None),
        }
    }

    /// Marks the rows from the selection to `id`, ends included.
    pub fn mark_range(&mut self, id: &TaskId) {
        let order = order(&self.snapshot);
        let at = |task: Option<&TaskId>| order.iter().position(|row| Some(row) == task);
        let (Some(from), Some(to)) = (at(self.selected.as_ref()), at(Some(id))) else {
            return;
        };
        let span = from.min(to)..=from.max(to);
        self.marks.extend(order[span].iter().cloned());
    }

    /// Marks every row, or clears them when all are marked.
    pub fn mark_visible(&mut self) {
        let order = order(&self.snapshot);
        if order.is_empty() {
            self.show_toast("no visible tasks to mark".to_string(), None);
        } else if order.iter().all(|id| self.marks.contains(id)) {
            self.marks.clear();
        } else {
            self.marks = order.into_iter().collect();
        }
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }

    pub fn select_series(&mut self, id: RecurrenceSeriesId) {
        if self.series.as_ref() != Some(&id) {
            self.series_detail = None;
        }
        self.series = Some(id.clone());
        self.send(Command::LoadSeries(id));
    }

    /// The series `delta` rows from the selected one.
    pub fn neighbour_series(&self, delta: isize) -> Option<RecurrenceSeriesId> {
        let rows = &self.snapshot.series;
        let last = rows.len().checked_sub(1)? as isize;
        let at = self
            .series
            .as_ref()
            .and_then(|id| rows.iter().position(|row| row.id == *id));
        let next = at.map_or(0, |at| (at as isize + delta).clamp(0, last));
        Some(rows[next as usize].id.clone())
    }

    pub fn start_review(&mut self) {
        self.send(Command::StartReview(Vec::new()));
    }

    pub fn next_review_page(&mut self) {
        if let Some(seen) = self.review.as_ref().map(|review| review.seen.clone()) {
            self.send(Command::StartReview(seen));
        }
    }

    fn take_review_page(&mut self, cards: Vec<ReviewCard>, more: usize) {
        if cards.is_empty() {
            self.show_toast("nothing else to triage".to_string(), None);
            return;
        }
        let seen = cards.iter().map(|card| card.id.clone());
        match &mut self.review {
            Some(review) => {
                review.seen.extend(seen);
                review.total_decided += review.decided.len();
                review.decided.clear();
                review.changes.clear();
                review.cards = cards;
                review.at = 0;
                review.more = more;
            }
            None => {
                self.review = Some(Review {
                    seen: seen.collect(),
                    cards,
                    at: 0,
                    more,
                    decided: BTreeSet::new(),
                    changes: Vec::new(),
                    last_key: None,
                    marks: std::mem::take(&mut self.marks),
                    total_decided: 0,
                });
                self.expanded = false;
            }
        }
        self.keep_card_open();
    }

    /// Shows the review's card through the details, so every task command
    /// reaches it. Between cards the details rest on the selected row.
    fn keep_card_open(&mut self) {
        let Some(review) = &self.review else {
            return;
        };
        let card = review.card().map(|card| card.id.clone());
        if card == self.peek {
            return;
        }
        self.detail = None;
        self.peek.clone_from(&card);
        self.trail.clear();
        if let Some(id) = card.or_else(|| self.selected.clone()) {
            self.last_detail = self.send(Command::Peek(id));
        }
    }

    fn review_decided(&mut self, change: ReviewChange) {
        if let Some(review) = &mut self.review {
            if let Some(card) = review.card() {
                review.decided.insert(card.id.clone());
            }
            review.changes.push(change);
            review.advance();
        }
        self.keep_card_open();
    }

    fn review_repeated(&mut self, key: char) -> bool {
        self.review
            .as_mut()
            .is_none_or(|review| review.repeated(key))
    }

    /// A status key decides the card. Its own status keeps a stale task and
    /// leaves an inbox task where it is, which needs a real destination.
    pub fn review_status(&mut self, key: char, status: TaskStatus) {
        if self.review_repeated(key) {
            return;
        }
        let Some(card) = self.review.as_ref().and_then(Review::card).cloned() else {
            return;
        };
        let Some(row) = self.target_row().cloned() else {
            return;
        };
        if row.status != status {
            let at = self.review.as_ref().map_or(0, |review| review.at);
            self.set_status(&card.id, status);
            self.review_decided(ReviewChange::Store(at));
        } else if status == TaskStatus::Inbox {
            self.show_toast("choose a destination for inbox tasks".to_string(), None);
        } else {
            self.review_keep_card();
        }
    }

    pub fn review_keep(&mut self) {
        if !self.review_repeated('k') {
            self.review_keep_card();
        }
    }

    fn review_keep_card(&mut self) {
        let Some(card) = self.review.as_ref().and_then(Review::card).cloned() else {
            return;
        };
        if !card.stale {
            self.show_toast("choose a destination for inbox tasks".to_string(), None);
            return;
        }
        let at = self.review.as_ref().map_or(0, |review| review.at);
        self.send(Command::Keep(card.id));
        self.review_decided(ReviewChange::Keep(at));
    }

    /// A change that refines the card and leaves it on screen.
    pub fn review_refined(&mut self) {
        if let Some(review) = &mut self.review {
            review.changes.push(ReviewChange::Store(review.at));
        }
    }

    /// Deferring takes a task out of the review's hands.
    pub fn review_deferred(&mut self) {
        let at = self.review.as_ref().map(|review| review.at);
        if let Some(at) = at {
            self.review_decided(ReviewChange::Store(at));
        }
    }

    pub fn review_skip(&mut self) {
        if !self.review_repeated(' ')
            && let Some(review) = &mut self.review
        {
            review.advance();
            self.keep_card_open();
        }
    }

    pub fn review_back(&mut self) {
        if let Some(review) = &mut self.review {
            review.at = review.at.saturating_sub(1);
            self.keep_card_open();
        }
    }

    /// Takes back the newest change and returns to its card.
    pub fn review_undo(&mut self) {
        let Some(review) = &mut self.review else {
            return;
        };
        let Some(change) = review.changes.pop() else {
            self.undo();
            return;
        };
        let (ReviewChange::Store(at) | ReviewChange::Keep(at)) = change;
        review.at = at;
        let id = review.cards[at].id.clone();
        review.decided.remove(&id);
        match change {
            ReviewChange::Store(_) => self.undo(),
            ReviewChange::Keep(_) => {
                self.send(Command::Unkeep(id));
            }
        }
        self.keep_card_open();
        if let Some(id) = self.peek.clone() {
            self.last_detail = self.send(Command::Peek(id));
        }
    }

    /// A decision that failed to save goes back on the table.
    fn review_save_failed(&mut self, command: &Command) {
        let (Command::SetStatus(id, _) | Command::Keep(id)) = command else {
            return;
        };
        let Some(review) = &mut self.review else {
            return;
        };
        if let Some(at) = review.cards.iter().position(|card| card.id == *id) {
            review.at = at;
            review.decided.remove(id);
            review.changes.pop();
        }
        self.keep_card_open();
    }

    pub fn exit_review(&mut self) {
        let Some(review) = self.review.take() else {
            return;
        };
        let decided = review.total_decided + review.decided.len();
        let undecided = review.seen.len() - decided;
        self.marks = review.marks;
        self.leave_peek();
        self.detail = None;
        if let Some(id) = self.selected.clone() {
            self.load_detail(id);
        }
        self.prune_marks();
        self.show_toast(
            format!("left triage · {decided} decided · {undecided} undecided"),
            None,
        );
    }

    fn prune_marks(&mut self) {
        if !self.marks.is_empty() {
            let shown: BTreeSet<TaskId> = order(&self.snapshot).into_iter().collect();
            self.marks.retain(|id| shown.contains(id));
        }
    }

    /// Publishes a report once at a time, as the TUI does.
    pub fn publish_gist(&mut self, id: TaskId) {
        if self.gist_running {
            self.show_toast("gist creation already in progress".to_string(), None);
            return;
        }
        self.gist_running = true;
        self.show_toast("creating secret gist".to_string(), None);
        self.send(Command::PublishGist(id));
    }

    /// Deletes a task, letting the list animate its row away.
    pub fn delete(&mut self, id: &TaskId) {
        let request = self.send(Command::SetDeleted(id.clone(), true));
        self.moving = Some((request, id.clone()));
    }

    /// Runs a write with no optimistic change.
    pub fn write(&mut self, command: Command) {
        self.send(command);
    }

    pub fn undo(&mut self) {
        self.send(Command::Undo);
    }

    /// The task `delta` rows from the selection in display order.
    pub fn neighbour(&self, delta: isize) -> Option<TaskId> {
        let order = order(&self.snapshot);
        let last = order.len().checked_sub(1)? as isize;
        let at = self
            .selected
            .as_ref()
            .and_then(|id| order.iter().position(|task| task == id));
        let next = at.map_or(0, |at| (at as isize + delta).clamp(0, last));
        Some(order[next as usize].clone())
    }

    pub fn set_status(&mut self, id: &TaskId, status: TaskStatus) {
        let Some(row) = self.snapshot.tasks.iter_mut().find(|row| &row.id == id) else {
            self.send(Command::SetStatus(id.clone(), status));
            return;
        };
        if row.status == status {
            return;
        }
        let previous = std::mem::replace(&mut row.status, status);
        self.motion.status_changed(id, previous, status);
        let request = self.send(Command::SetStatus(id.clone(), status));
        self.moving = Some((request, id.clone()));
    }

    pub fn toggle_status(&mut self, id: &TaskId, status: TaskStatus) {
        let row = self.snapshot.row(id).or(self.target_row());
        let Some(row) = row.filter(|row| row.id == *id) else {
            return;
        };
        let next = if row.status == status {
            TaskStatus::Todo
        } else {
            status
        };
        self.set_status(id, next);
    }

    pub fn set_priority(&mut self, id: &TaskId, priority: TaskPriority) {
        let Some(row) = self.snapshot.tasks.iter_mut().find(|row| &row.id == id) else {
            self.send(Command::SetPriority(id.clone(), priority));
            return;
        };
        if row.priority == priority {
            return;
        }
        let previous = std::mem::replace(&mut row.priority, priority);
        self.motion.priority_changed(id, previous, priority);
        let request = self.send(Command::SetPriority(id.clone(), priority));
        self.moving = Some((request, id.clone()));
    }

    pub fn set_title(&mut self, id: &TaskId, title: String) {
        let Some(row) = self.snapshot.tasks.iter_mut().find(|row| &row.id == id) else {
            self.send(Command::SetTitle(id.clone(), title));
            return;
        };
        if row.title == title {
            return;
        }
        row.title = title.clone();
        self.send(Command::SetTitle(id.clone(), title));
    }

    pub fn add_note(&mut self, id: &TaskId, body: String) {
        self.note_drafts.remove(id);
        self.send(Command::AddNote(id.clone(), body));
    }

    pub fn create(&mut self, task: NewTask) {
        self.create_from = self.selected.clone();
        self.send(Command::Create(Box::new(task)));
    }

    pub fn intake(&mut self, input: String, project: Option<String>) {
        let project = project.or_else(|| self.snapshot.view.project.clone());
        self.send(Command::Intake { input, project });
    }

    /// A refresh the user asked for: the Done group empties.
    pub fn refresh_view(&mut self) {
        self.done.clear();
        self.moving = None;
        self.send(Command::RefreshView);
    }

    pub fn run_toast_action(&mut self) {
        match self.toast.take().and_then(|toast| toast.action) {
            Some(ToastAction::Retry(command)) => {
                self.send(command);
            }
            Some(ToastAction::Refresh) => {
                self.send(Command::Refresh);
                if let Some(id) = self.selected.clone() {
                    self.load_detail(id);
                }
            }
            Some(ToastAction::Go(go)) => match go.command {
                Command::Goto(id) => self.goto(id),
                command => self.switch_view(command),
            },
            Some(ToastAction::RestoreDraft) | None => {}
        }
    }

    pub fn run_toast_undo(&mut self) {
        self.toast = None;
        self.undo();
    }

    pub fn show_toast(&mut self, message: String, action: Option<ToastAction>) {
        let run = self.motion.next_run();
        self.toast = Some(Toast {
            run,
            message,
            action,
            undo: false,
        });
    }

    /// Reports a copy without touching the row, which only the ref copy marks.
    pub fn copied_toast(&mut self, what: &str) {
        self.show_toast(format!("Copied {what}"), None);
    }

    /// Puts text typed while a failed note was on its way after the note.
    pub fn merge_note_draft(&mut self, id: &TaskId, typed: &str) {
        let draft = self.note_drafts.entry(id.clone()).or_default();
        *draft = join_drafts(draft, typed);
    }

    /// Takes one reply. Lists or details that a newer request has overtaken
    /// change nothing.
    pub fn apply(&mut self, reply: Reply) -> Applied {
        let mut applied = Applied::default();
        match reply.body {
            ReplyBody::List(list) => {
                // A list that predates a write would undo its optimistic change.
                if reply.request >= self.last_mutation {
                    applied.moved = self
                        .moving
                        .take_if(|(request, _)| *request <= reply.request)
                        .map(|(_, id)| id);
                    self.take_list(*list, &mut applied);
                }
            }
            ReplyBody::Detail(detail) => {
                let current =
                    reply.request == self.last_detail && self.target() == Some(&detail.task_id);
                if current && self.detail.as_ref() != Some(&detail) {
                    self.detail = Some(*detail);
                }
            }
            ReplyBody::Series(detail) => {
                if self.series.as_ref() == Some(&detail.id) {
                    self.series_detail = Some(*detail);
                }
            }
            ReplyBody::SelectSeries(id) => {
                self.select_series(id);
                applied.series = true;
            }
            ReplyBody::Review(cards, more) => self.take_review_page(cards, more),
            ReplyBody::AskStop(id) => applied.stop = Some(id),
            ReplyBody::Gist(result) => {
                self.gist_running = false;
                match result {
                    Ok(url) => {
                        applied.copy = Some(url);
                        let message = "created secret gist and copied its URL";
                        self.show_toast(message.to_string(), None);
                    }
                    Err(message) => self.show_toast(message, None),
                }
            }
            ReplyBody::Catalog(catalog) => self.catalog = catalog,
            ReplyBody::Search(results) => {
                if reply.request == self.last_search {
                    self.search = results;
                }
            }
            ReplyBody::Copied(text) => {
                self.copied_toast("task report");
                applied.copy = Some(text);
            }
            ReplyBody::Intake(task) => applied.intake = Some(task),
            ReplyBody::IntakeFailed(message) => {
                applied.intake_failed = true;
                self.show_toast(format!("Could not read that: {message}"), None);
            }
            ReplyBody::Created(id) => {
                // Moving on while the write ran keeps the newer selection.
                if self.selected == self.create_from {
                    self.select(id);
                    applied.selection = true;
                }
            }
            ReplyBody::Select(id) => {
                self.leave_peek();
                self.selected = None;
                self.select(id);
                applied.selection = true;
            }
            ReplyBody::Saved { message, go, undo } => {
                self.show_toast(message, go.map(ToastAction::Go));
                if let Some(toast) = &mut self.toast {
                    toast.undo = undo;
                }
                // Details of a peeked task are not refreshed by its list.
                if let Some(id) = self.peek.clone() {
                    self.last_detail = self.send(Command::Peek(id));
                }
            }
            ReplyBody::Notice(message) => self.show_toast(message, None),
            ReplyBody::SaveFailed(Command::AddNote(id, body), message) => {
                applied.draft = self.selected.as_ref() == Some(&id);
                let draft = self.note_drafts.entry(id).or_default();
                *draft = join_drafts(&body, draft);
                self.show_toast(format!("Could not save the note: {message}"), None);
            }
            ReplyBody::SaveFailed(Command::Create(task), message) => {
                applied.composer_draft = Some(task);
                self.show_toast(
                    format!("Could not create the task, it is back in New task: {message}"),
                    None,
                );
            }
            ReplyBody::SaveFailed(command, message) => {
                self.review_save_failed(&command);
                self.show_toast(
                    format!("Could not save: {message}"),
                    Some(ToastAction::Retry(command)),
                );
            }
            ReplyBody::SavedRefreshFailed(message) => {
                self.show_toast(
                    format!("Saved, but the list did not refresh: {message}"),
                    Some(ToastAction::Refresh),
                );
            }
            ReplyBody::RefreshFailed(message) => {
                self.show_toast(
                    format!("Could not refresh: {message}"),
                    Some(ToastAction::Refresh),
                );
            }
            ReplyBody::Sync(health) => self.sync = Some(health),
        }
        self.keep_card_open();
        applied
    }

    /// Moves closed rows to one last group. A row the store no longer lists keeps
    /// the fields it had, and a row reopened anywhere rejoins its own group.
    fn gather_done(&mut self, list: &mut Snapshot) {
        if list.view.lists_closed() {
            self.done.clear();
            return;
        }
        self.done
            .retain(|id| list.row(id).is_none_or(|row| row.status.is_terminal()));
        for group in &mut list.groups {
            group.rows.retain(|task| {
                let row = &list.tasks[*task];
                if !row.status.is_terminal() {
                    return true;
                }
                if !self.done.contains(&row.id) {
                    self.done.push(row.id.clone());
                }
                false
            });
        }
        list.groups.retain(|group| !group.rows.is_empty());
        for id in &self.done {
            if list.row(id).is_none()
                && let Some(row) = self.snapshot.row(id)
            {
                list.tasks.push(row.clone());
            }
        }
        let rows: Vec<usize> = self
            .done
            .iter()
            .filter_map(|id| list.tasks.iter().position(|row| row.id == *id))
            .collect();
        if !rows.is_empty() {
            list.groups.push(done_group(rows));
        }
    }

    /// Whether a list with nothing open is what a local write left behind.
    pub fn all_clear(&self) -> bool {
        let groups = &self.snapshot.groups;
        groups.len() == 1
            && groups[0]
                .rows
                .iter()
                .all(|row| self.snapshot.tasks[*row].status.is_terminal())
            && !self.done.is_empty()
    }

    /// Starts what a local write's list earns: rings for epics whose children
    /// moved, and All clear for the row that was the last.
    fn start_list_motion(&mut self, before: &Snapshot) {
        for row in &self.snapshot.tasks {
            let was = before.row(&row.id).and_then(|row| row.epic.as_ref());
            if let (Some(now), Some(was)) = (&row.epic, was)
                && now.total() - now.open != was.total() - was.open
            {
                self.motion.ring_changed(&row.id, was.total() - was.open);
            }
        }
        let open = before.ordered().any(|ix| {
            !before.tasks[ix].status.is_terminal() || !self.done.contains(&before.tasks[ix].id)
        });
        if open && self.all_clear() {
            self.motion.cleared();
        }
    }

    /// Keeps a series selected while the Recurring view lists any, reloading it
    /// since the write that produced this list may have changed it.
    fn keep_series(&mut self) {
        if self.snapshot.view.query != TaskQuery::Recurring {
            self.series = None;
            self.series_detail = None;
            return;
        }
        let kept = self
            .series
            .clone()
            .filter(|id| self.snapshot.series.iter().any(|row| row.id == *id));
        let next = kept.or_else(|| self.snapshot.series.first().map(|row| row.id.clone()));
        match next {
            Some(id) => self.select_series(id),
            None => {
                self.series = None;
                self.series_detail = None;
            }
        }
    }

    /// The row in a locally completed selection's place, staying on open rows.
    fn after_completion(&self, id: &TaskId, was: &[TaskId]) -> Option<TaskId> {
        let at = was.iter().position(|task| task == id)?;
        let open = |task: &&TaskId| !self.done.contains(task);
        let next = was[at + 1..].iter().find(open);
        next.or_else(|| was[..at].iter().rfind(open)).cloned()
    }

    fn take_list(&mut self, mut list: Snapshot, applied: &mut Applied) {
        let already_done = (self.selected.as_ref()).is_some_and(|id| self.done.contains(id));
        self.gather_done(&mut list);
        applied.list = list != self.snapshot;
        let completed = applied.moved.clone().filter(|id| {
            !already_done && self.selected.as_ref() == Some(id) && self.done.contains(id)
        });
        let replaced = std::mem::replace(&mut self.snapshot, list);
        if applied.moved.is_some() {
            self.start_list_motion(&replaced);
            let gone = replaced
                .tasks
                .iter()
                .filter(|row| self.snapshot.row(&row.id).is_none());
            applied.left = gone.cloned().collect();
        }
        self.prune_marks();
        self.keep_series();
        if let Some(next) = completed.and_then(|id| self.after_completion(&id, &order(&replaced))) {
            self.select(next);
            applied.selection = true;
            return;
        }
        let kept = self
            .selected
            .clone()
            .filter(|id| self.snapshot.row(id).is_some());
        match (kept, self.selected.clone()) {
            (Some(id), _) if self.peek.is_none() => self.load_detail(id),
            (Some(_), _) => {}
            // A selected task that left moves to the row that took its place.
            (None, Some(gone)) => {
                let at = order(&replaced).iter().position(|id| *id == gone);
                let order = order(&self.snapshot);
                let next = order.get(at.unwrap_or(0).min(order.len().saturating_sub(1)));
                self.selected = None;
                self.detail = None;
                if let Some(next) = next {
                    self.select(next.clone());
                }
                applied.selection = true;
            }
            (None, None) => {
                if let Some(id) = first_row(&self.snapshot) {
                    self.select(id);
                    applied.selection = true;
                }
            }
        }
    }
}

fn order(snapshot: &Snapshot) -> Vec<TaskId> {
    snapshot
        .ordered()
        .map(|ix| snapshot.tasks[ix].id.clone())
        .collect()
}

fn first_row(snapshot: &Snapshot) -> Option<TaskId> {
    let first = snapshot.ordered().next()?;
    Some(snapshot.tasks[first].id.clone())
}

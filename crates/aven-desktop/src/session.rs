//! The app's state above the store: selection, drafts, request identity, messages
//! and motion bookkeeping. It has no GPUI types, so behaviour tests need no window.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::ids::TaskId;
use tokio::sync::mpsc::UnboundedSender;

use crate::adapter::{
    CatalogTask, Command, Detail, Go, NewTask, Reply, ReplyBody, Request, SearchInput,
    SearchResults, Snapshot, SyncHealth, TaskRow, done_group,
};

pub const POP_DURATION: Duration = Duration::from_millis(320);
pub const SETTLE_DURATION: Duration = Duration::from_millis(620);
pub const PRIORITY_DURATION: Duration = Duration::from_millis(200);
pub const COPIED_DURATION: Duration = Duration::from_millis(1300);

/// Motions keyed on wall time. Each expires on its own, so a row scrolled back
/// into view does not replay it, and a refresh never starts one.
#[derive(Debug, Default)]
pub struct Motion {
    reduced: bool,
    runs: usize,
    completed: HashMap<TaskId, (TaskStatus, usize, Instant)>,
    repriced: HashMap<TaskId, (TaskPriority, usize, Instant)>,
    copied: Option<(TaskId, usize, Instant)>,
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
    pub copy: Option<(String, &'static str)>,
    pub search: bool,
    /// The task whose local write produced this list, so the list may animate.
    pub moved: Option<TaskId>,
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

fn is_closed(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Done | TaskStatus::Canceled)
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
            ReplyBody::Catalog(catalog) => self.catalog = catalog,
            ReplyBody::Search(results) => {
                if reply.request == self.last_search {
                    self.search = results;
                    applied.search = true;
                }
            }
            ReplyBody::Copied(text, what) => {
                self.copied_toast(what);
                applied.copy = Some((text, what));
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
            .retain(|id| list.row(id).is_none_or(|row| is_closed(row.status)));
        for group in &mut list.groups {
            group.rows.retain(|task| {
                let row = &list.tasks[*task];
                if !is_closed(row.status) {
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

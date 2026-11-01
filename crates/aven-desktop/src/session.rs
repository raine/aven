//! The app's state above the store: selection, drafts, request identity, messages
//! and motion bookkeeping. It has no GPUI types, so behaviour tests need no window.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::ids::TaskId;
use tokio::sync::mpsc::UnboundedSender;

use crate::adapter::{
    Command, Detail, Reply, ReplyBody, Request, Snapshot, SyncHealth, done_group,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub run: usize,
    pub message: String,
    pub action: Option<ToastAction>,
}

/// What a reply changed, for the view.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub list: bool,
    pub selection: bool,
    /// A failed note went back into the selected task's draft.
    pub draft: bool,
    /// The title of a task that could not be created.
    pub composer_draft: Option<String>,
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
    pub detail: Option<Detail>,
    pub note_drafts: HashMap<TaskId, String>,
    pub toast: Option<Toast>,
    pub sync: Option<SyncHealth>,
    pub motion: Motion,
    /// Tasks completed during this visit to the view, oldest first. They stay
    /// in the Done group until the user refreshes the view.
    done: Vec<TaskId>,
    /// The newest status or priority write whose list has not arrived.
    moving: Option<(u64, TaskId)>,
    requests: UnboundedSender<Request>,
    next_request: u64,
    last_mutation: u64,
    last_detail: u64,
    /// The selection when the newest create was sent.
    create_from: Option<TaskId>,
}

impl Session {
    pub fn new(snapshot: Snapshot, requests: UnboundedSender<Request>) -> Self {
        let selected = first_row(&snapshot);
        let mut this = Self {
            snapshot,
            selected: None,
            detail: None,
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
        if self.selected.as_ref() == Some(&id) {
            return;
        }
        self.selected = Some(id.clone());
        self.detail = None;
        self.load_detail(id);
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
        let Some(row) = self.snapshot.row(id) else {
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

    pub fn create(&mut self, title: String) {
        self.create_from = self.selected.clone();
        self.send(Command::Create(title));
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
            None => {}
        }
    }

    fn show_toast(&mut self, message: String, action: Option<ToastAction>) {
        let run = self.motion.next_run();
        self.toast = Some(Toast {
            run,
            message,
            action,
        });
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
                let current = reply.request == self.last_detail
                    && self.selected.as_ref() == Some(&detail.task_id);
                if current && self.detail.as_ref() != Some(&detail) {
                    self.detail = Some(*detail);
                }
            }
            ReplyBody::Created(id) => {
                // Moving on while the write ran keeps the newer selection.
                if self.selected == self.create_from {
                    self.select(id);
                    applied.selection = true;
                }
            }
            ReplyBody::SaveFailed(Command::AddNote(id, body), message) => {
                applied.draft = self.selected.as_ref() == Some(&id);
                let draft = self.note_drafts.entry(id).or_default();
                *draft = join_drafts(&body, draft);
                self.show_toast(format!("Could not save the note: {message}"), None);
            }
            ReplyBody::SaveFailed(Command::Create(title), message) => {
                applied.composer_draft = Some(title);
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

    /// Moves done rows to one last group. A row the store no longer lists keeps
    /// the fields it had, and a row reopened anywhere rejoins its own group.
    fn gather_done(&mut self, list: &mut Snapshot) {
        self.done.retain(|id| {
            list.row(id)
                .is_none_or(|row| row.status == TaskStatus::Done)
        });
        for group in &mut list.groups {
            group.rows.retain(|task| {
                let row = &list.tasks[*task];
                if row.status != TaskStatus::Done {
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
        applied.list = list.tasks != self.snapshot.tasks || list.groups != self.snapshot.groups;
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
            (Some(id), _) => self.load_detail(id),
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

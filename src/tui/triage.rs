//! Triage mode: walks inbox and stale open tasks one card at a time in short
//! pages. Keys resolve through the normal command catalog, and a card advances
//! only when the command's intent is a decision about the task.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use anyhow::Result;
use aven_core::query::{
    SortDirection, TaskAvailabilityFilter, TaskFilters, TaskListItem, TaskQueryMode, TaskSort,
};
use aven_core::queue::{
    QueueDateKind, STALE_ACTIVE_DAYS, STALE_TODO_DAYS, TriageReason, now_seconds, triage_reason,
    triage_review_current, unix_seconds,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::choices::{TaskPriority, TaskStatus};
use crate::ids::TaskId;
use crate::tui::app::App;
use crate::tui::event::{Action, CommandHandler, RoutingDomain};
use crate::tui::input::key::{NormalKeyInput, route_normal_key_in_domain};
use crate::tui::store::{MainRowSelection, TaskQuery, TaskScope, TaskViewState, TuiStore};

pub(crate) const PAGE_SIZE: usize = 10;

/// A repeat of the key that just decided a card arrives faster than a person
/// reads the next card, so it is treated as key repeat rather than a decision.
const REPEAT_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriageOutcome {
    Status(TaskStatus),
    Kept(TaskStatus),
    Deferred,
    Deleted,
}

impl TriageOutcome {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Status(status) => format!("→ {status}"),
            Self::Kept(status) => format!("kept {status}"),
            Self::Deferred => "deferred".to_string(),
            Self::Deleted => "deleted".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TriageEntry {
    pub(crate) item: TaskListItem,
    pub(crate) reason: TriageReason,
    pub(crate) idle_days: i64,
    pub(crate) outcome: Option<TriageOutcome>,
    /// The user moved past this card without deciding it.
    pub(crate) passed: bool,
}

impl TriageEntry {
    fn new(item: TaskListItem, reason: TriageReason) -> Self {
        Self {
            idle_days: item.queue.idle_days.unwrap_or(0),
            item,
            reason,
            outcome: None,
            passed: false,
        }
    }

    fn pressing(&self) -> bool {
        self.item.task.priority == TaskPriority::Urgent
            || matches!(
                self.item.queue.date_kind,
                Some(QueueDateKind::Overdue | QueueDateKind::DueToday)
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriageIntent {
    /// The command decides what happens to the task; the card advances when it
    /// closes, moves out of open work, or defers the task.
    Decide,
    /// The command refines the task; the card stays.
    Edit,
}

#[derive(Debug, Clone)]
struct PendingCommand {
    index: usize,
    intent: TriageIntent,
    status_before: TaskStatus,
    undo_before: Option<String>,
    key: Option<KeyCode>,
}

/// One undoable change made from a triage card.
#[derive(Debug, Clone)]
struct TriageChange {
    index: usize,
    undo: ChangeUndo,
    previous: Option<TriageOutcome>,
}

#[derive(Debug, Clone)]
enum ChangeUndo {
    /// A task mutation, reverted through the store's undo entry.
    Store(String),
    /// A local review mark, reverted by restoring the mark it replaced.
    Review {
        task_id: TaskId,
        previous: Option<i64>,
    },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectCounts {
    pub(crate) todo: usize,
    pub(crate) active: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TriageSession {
    pub(crate) entries: Vec<TriageEntry>,
    /// Index into `entries`; equal to `entries.len()` on the end screen.
    pub(crate) cursor: usize,
    /// Eligible tasks when the page was built, including the page itself.
    pub(crate) eligible: usize,
    pub(crate) more_available: bool,
    pub(crate) project_counts: BTreeMap<String, ProjectCounts>,
    /// Project the session is limited to, from the scope triage opened in.
    pub(crate) project: Option<String>,
    shown: BTreeSet<TaskId>,
    changes: Vec<TriageChange>,
    pending: Option<PendingCommand>,
    /// A custom command launched from a card, settled when it finishes.
    custom: Option<PendingCommand>,
    synced_undo: Option<String>,
    last_decision: Option<(KeyCode, Instant)>,
    previous_view: TaskViewState,
    previous_selection: Option<TaskId>,
    previous_marks: BTreeSet<TaskId>,
    /// Body scroll of the card at `scroll_card`; other cards start at the top.
    scroll_card: usize,
    scroll: u16,
    /// Scroll limit and visible body height from the last render.
    pub(crate) scroll_max: Cell<u16>,
    pub(crate) scroll_viewport: Cell<u16>,
}

impl TriageSession {
    pub(crate) fn card_scroll(&self) -> u16 {
        if self.scroll_card == self.cursor {
            self.scroll.min(self.scroll_max.get())
        } else {
            0
        }
    }

    fn scroll_by(&mut self, delta: i32) {
        let current = self.card_scroll();
        self.scroll_card = self.cursor;
        self.scroll =
            (i32::from(current) + delta).clamp(0, i32::from(self.scroll_max.get())) as u16;
    }

    fn scroll_page(&mut self, direction: i32, half: bool) {
        let viewport = i32::from(self.scroll_viewport.get().max(2));
        let page = if half { viewport / 2 } else { viewport - 1 };
        self.scroll_by(direction * page.max(1));
    }

    pub(crate) fn current(&self) -> Option<&TriageEntry> {
        self.entries.get(self.cursor)
    }

    pub(crate) fn at_end(&self) -> bool {
        self.cursor >= self.entries.len()
    }

    pub(crate) fn decided_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.outcome.is_some())
            .count()
    }

    pub(crate) fn eligible_now(&self) -> usize {
        self.eligible.saturating_sub(self.decided_count())
    }

    /// The most recent decisions, newest first.
    pub(crate) fn recent_decisions(&self) -> impl Iterator<Item = &TriageEntry> {
        let mut seen = BTreeSet::new();
        self.changes
            .iter()
            .rev()
            .filter(move |change| seen.insert(change.index))
            .map(|change| &self.entries[change.index])
            .filter(|entry| entry.outcome.is_some())
    }

    fn next_undecided_after(&self, index: usize) -> usize {
        (index + 1..self.entries.len())
            .find(|candidate| self.entries[*candidate].outcome.is_none())
            .unwrap_or(self.entries.len())
    }

    fn step(&mut self, forward: bool) {
        if let Some(entry) = self.entries.get_mut(self.cursor)
            && entry.outcome.is_none()
        {
            entry.passed = true;
        }
        self.cursor = if forward {
            (self.cursor + 1).min(self.entries.len())
        } else {
            self.cursor.saturating_sub(1)
        };
    }
}

/// Orders one page: urgent or due work first, then one task from each pool in
/// turn so inbox intake cannot starve stale commitments or the reverse.
pub(crate) fn build_page(mut candidates: Vec<TriageEntry>) -> Vec<TriageEntry> {
    candidates.sort_by(|left, right| {
        left.item
            .task
            .project_key
            .cmp(&right.item.task.project_key)
            .then(right.idle_days.cmp(&left.idle_days))
    });
    let (mut page, rest): (Vec<_>, Vec<_>) =
        candidates.into_iter().partition(TriageEntry::pressing);
    page.truncate(PAGE_SIZE);
    let mut pools = [
        TriageReason::StaleActive,
        TriageReason::Inbox,
        TriageReason::StaleTodo,
    ]
    .map(|_| VecDeque::new());
    for entry in rest {
        let pool = match entry.reason {
            TriageReason::StaleActive => 0,
            TriageReason::Inbox => 1,
            TriageReason::StaleTodo => 2,
        };
        pools[pool].push_back(entry);
    }
    while page.len() < PAGE_SIZE && pools.iter().any(|pool| !pool.is_empty()) {
        for pool in &mut pools {
            if page.len() < PAGE_SIZE
                && let Some(entry) = pool.pop_front()
            {
                page.push(entry);
            }
        }
    }
    page
}

struct Candidates {
    entries: Vec<TriageEntry>,
    project_counts: BTreeMap<String, ProjectCounts>,
}

async fn load_candidates(
    store: &TuiStore,
    project: Option<&str>,
    exclude: &BTreeSet<TaskId>,
) -> Result<Candidates> {
    let items = store
        .database()
        .list_task_items(
            &store.active_workspace.id,
            TaskFilters {
                project: project.map(str::to_string),
                statuses: ["inbox", "todo", "active"].map(str::to_string).to_vec(),
                exclude_epics: true,
                availability: TaskAvailabilityFilter::Available,
                ..TaskFilters::default()
            },
            TaskQueryMode::Flat,
            TaskSort::Created,
            SortDirection::Asc,
        )
        .await?;
    let reviews = store.database().triage_reviews().await?;
    let now = now_seconds();
    let mut project_counts = BTreeMap::<String, ProjectCounts>::new();
    let mut entries = Vec::new();
    for item in items {
        let counts = project_counts
            .entry(item.task.project_key.clone())
            .or_default();
        match item.task.status {
            TaskStatus::Todo => counts.todo += 1,
            TaskStatus::Active => counts.active += 1,
            _ => {}
        }
        if exclude.contains(&item.task.id) {
            continue;
        }
        if let Some(reason) = triage_reason(&item.task, &item.queue, now)
            && !triage_review_current(reason, reviews.get(item.task.id.as_str()).copied(), now)
        {
            entries.push(TriageEntry::new(item, reason));
        }
    }
    Ok(Candidates {
        entries,
        project_counts,
    })
}

/// The task list's paging keys, which scroll the card body in triage.
fn card_page_key(key: KeyEvent) -> Option<(i32, bool)> {
    match (key.code, key.modifiers) {
        (KeyCode::PageDown, KeyModifiers::NONE) => Some((1, false)),
        (KeyCode::PageUp, KeyModifiers::NONE) => Some((-1, false)),
        (KeyCode::Char('d'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            Some((1, true))
        }
        (KeyCode::Char('u'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            Some((-1, true))
        }
        _ => None,
    }
}

fn deferred(item: &TaskListItem) -> bool {
    item.task
        .available_at
        .as_deref()
        .and_then(unix_seconds)
        .is_some_and(|available_at| available_at > now_seconds())
}

fn decision_outcome(
    item: Option<&TaskListItem>,
    pending: &PendingCommand,
) -> Option<TriageOutcome> {
    match (item, pending.intent) {
        (None, TriageIntent::Decide) => Some(TriageOutcome::Deleted),
        (Some(item), TriageIntent::Decide) if item.task.deleted => Some(TriageOutcome::Deleted),
        (Some(item), TriageIntent::Decide) if item.task.status != pending.status_before => {
            Some(TriageOutcome::Status(item.task.status))
        }
        (Some(item), TriageIntent::Decide) if deferred(item) => Some(TriageOutcome::Deferred),
        _ => None,
    }
}

fn command_intent(action: Action) -> Option<TriageIntent> {
    match action {
        Action::SetStatus(_)
        | Action::BeginStatusPicker
        | Action::BeginEditAvailability
        | Action::Delete => Some(TriageIntent::Decide),
        Action::SetPriority(_)
        | Action::CyclePriority(_)
        | Action::BeginEditPriority
        | Action::BeginEditLabels
        | Action::BeginEditDescription
        | Action::BeginEditProject
        | Action::BeginEditDue
        | Action::BeginEditMetadata
        | Action::BeginAddNote
        | Action::CopyShortRef
        | Action::CopyDurableRef
        | Action::CopyTaskTitle
        | Action::CopyTaskDescription
        | Action::CopyTaskText
        | Action::CopyTaskNotes
        | Action::CopyTaskMarkdown => Some(TriageIntent::Edit),
        _ => None,
    }
}

impl App {
    pub(crate) async fn open_triage(&mut self) -> Result<()> {
        // A project scope limits triage to that project.
        let project = match &self.store.view_state.scope {
            TaskScope::Project(key) => Some(key.clone()),
            TaskScope::Workspace => None,
        };
        let candidates = load_candidates(&self.store, project.as_deref(), &BTreeSet::new()).await?;
        if candidates.entries.is_empty() {
            self.set_info(match &project {
                Some(key) => format!("nothing to triage in {key}"),
                None => "nothing to triage".to_string(),
            });
            return Ok(());
        }
        let eligible = candidates.entries.len();
        let entries = build_page(candidates.entries);
        let previous_selection = self
            .store
            .selected_task(self.list.selected_task())
            .map(|item| item.task.id.clone());
        let previous_marks = self.list.marked_task_ids().clone();
        let previous_view = self.store.view_state.clone();
        self.pending_shortcut.clear();
        self.detail = crate::tui::detail_session::DetailSession::inactive();
        self.list.clear_marks();
        self.list.focus_tasks();
        // Commands act on the list selection, so the list shows every task in
        // the triage scope while triage runs and the selection follows the card.
        let triage_view = TaskViewState {
            scope: project
                .clone()
                .map_or(TaskScope::Workspace, TaskScope::Project),
            query: TaskQuery::All,
            ..TaskViewState::default()
        };
        self.store.restore_view_state(triage_view, None).await?;
        self.triage = Some(TriageSession {
            more_available: eligible > entries.len(),
            shown: entries
                .iter()
                .map(|entry| entry.item.task.id.clone())
                .collect(),
            entries,
            cursor: 0,
            eligible,
            project_counts: candidates.project_counts,
            project,
            changes: Vec::new(),
            pending: None,
            custom: None,
            synced_undo: self.latest_undo_id(),
            last_decision: None,
            previous_view,
            previous_selection,
            previous_marks,
            scroll_card: 0,
            scroll: 0,
            scroll_max: Cell::new(0),
            scroll_viewport: Cell::new(0),
        });
        self.notification = None;
        self.reload_triage_card().await
    }

    async fn next_triage_page(&mut self) -> Result<()> {
        let session = self.triage.as_ref().expect("triage session");
        let candidates =
            load_candidates(&self.store, session.project.as_deref(), &session.shown).await?;
        if candidates.entries.is_empty() {
            self.set_info("nothing else to triage");
            return Ok(());
        }
        let eligible = candidates.entries.len();
        let entries = build_page(candidates.entries);
        let session = self.triage.as_mut().expect("triage session");
        session
            .shown
            .extend(entries.iter().map(|entry| entry.item.task.id.clone()));
        session.more_available = eligible > entries.len();
        session.eligible = eligible;
        session.entries = entries;
        session.cursor = 0;
        session.project_counts = candidates.project_counts;
        session.changes.clear();
        session.custom = None;
        session.last_decision = None;
        self.reload_triage_card().await
    }

    async fn leave_triage(&mut self) -> Result<()> {
        let Some(session) = self.triage.take() else {
            return Ok(());
        };
        let decided = session.decided_count();
        let undecided = session.entries.len() - decided;
        let selected = session
            .current()
            .filter(|entry| entry.outcome.is_none())
            .map(|entry| entry.item.task.id.clone())
            .or(session.previous_selection);
        self.pending_shortcut.clear();
        let selected = selected.map(MainRowSelection::Task);
        let selected = self
            .store
            .restore_view_state(session.previous_view, selected.as_ref())
            .await?
            .selected;
        self.list.select_task(selected);
        self.list.mark_all(session.previous_marks);
        self.set_info(format!(
            "left triage · {decided} decided · {undecided} undecided"
        ));
        Ok(())
    }

    fn latest_undo_id(&self) -> Option<String> {
        self.store
            .latest_undo
            .as_ref()
            .map(|undo| undo.entry_id.clone())
    }

    /// Reloads the card's task with full detail and points the list selection
    /// at it so catalog commands target the card.
    async fn reload_triage_card(&mut self) -> Result<()> {
        let Some(task_id) = self
            .triage
            .as_ref()
            .and_then(TriageSession::current)
            .map(|entry| entry.item.task.id.clone())
        else {
            return Ok(());
        };
        if let Some(item) = self.store.load_task_item(&task_id).await? {
            let session = self.triage.as_mut().expect("triage session");
            let entry = &mut session.entries[session.cursor];
            entry.item = item;
        }
        if !self.select_task_by_id(&task_id) {
            let selected = self.store.refresh(Some(&task_id)).await?;
            self.list.select_task(selected);
        }
        Ok(())
    }

    pub(super) async fn handle_triage_key(&mut self, key: KeyEvent) -> Result<()> {
        let session = self.triage.as_mut().expect("triage session");
        let single_key = self.pending_shortcut.is_empty();
        if let Some((code, at)) = session.last_decision {
            if single_key && key.code == code && at.elapsed() < REPEAT_WINDOW {
                session.last_decision = Some((code, Instant::now()));
                return Ok(());
            }
            session.last_decision = None;
        }
        if session.at_end() {
            return self.handle_triage_end_key(key.code).await;
        }
        if single_key && let Some((direction, half)) = card_page_key(key) {
            session.scroll_page(direction, half);
            return Ok(());
        }
        let translation = route_normal_key_in_domain(
            &self.pending_shortcut,
            key.code,
            false,
            &self.command_catalog,
            RoutingDomain::Normal,
        );
        self.pending_shortcut = translation.shortcut;
        self.pending_shortcut_scroll = 0;
        match translation.input {
            NormalKeyInput::Command(CommandHandler::BuiltIn(action)) => {
                let key = (single_key && key.kind != KeyEventKind::Repeat).then_some(key.code);
                self.run_triage_action(action, key).await
            }
            NormalKeyInput::Command(handler @ CommandHandler::Custom(_)) => {
                self.run_triage_custom_command(handler).await
            }
            NormalKeyInput::CancelOverlay => self.leave_triage().await,
            NormalKeyInput::Missing(label) => {
                self.set_warning(format!("invalid shortcut: {label}"));
                Ok(())
            }
            NormalKeyInput::Overlay(_)
            | NormalKeyInput::CancelShortcut
            | NormalKeyInput::Prefix => Ok(()),
        }
    }

    /// Scrolls the card body by `lines`; returns false when triage does not
    /// own the screen.
    pub(super) fn scroll_triage_card(&mut self, lines: i32) -> bool {
        if self.overlay.is_some() || !self.detail.is_inactive() {
            return false;
        }
        let Some(session) = self.triage.as_mut() else {
            return false;
        };
        session.scroll_by(lines);
        true
    }

    async fn handle_triage_end_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Enter => self.next_triage_page().await,
            KeyCode::Esc | KeyCode::Char('q') => self.leave_triage().await,
            KeyCode::Char('u') => self.undo_triage_change().await,
            KeyCode::Char('k' | 'h') | KeyCode::Up | KeyCode::Left => {
                let session = self.triage.as_mut().expect("triage session");
                session.step(false);
                self.reload_triage_card().await
            }
            _ => Ok(()),
        }
    }

    async fn run_triage_action(&mut self, action: Action, key: Option<KeyCode>) -> Result<()> {
        match action {
            Action::MoveDown
            | Action::NextItem
            | Action::MoveRight
            | Action::MoveUp
            | Action::PreviousItem
            | Action::MoveLeft => {
                let session = self.triage.as_mut().expect("triage session");
                session.step(matches!(
                    action,
                    Action::MoveDown | Action::NextItem | Action::MoveRight
                ));
                return self.reload_triage_card().await;
            }
            Action::Quit => return self.leave_triage().await,
            Action::Undo => return self.undo_triage_change().await,
            Action::ToggleHelp | Action::ToggleDetail => return self.execute(action).await,
            _ => {}
        }
        let Some(intent) = command_intent(action) else {
            self.set_warning("not available in triage");
            return Ok(());
        };
        let session = self.triage.as_ref().expect("triage session");
        let index = session.cursor;
        let entry = &session.entries[index];
        let status = entry.item.task.status;
        if let Action::SetStatus(target) = action
            && target == status
        {
            if status == TaskStatus::Inbox {
                self.set_info("choose a destination for inbox tasks");
                return Ok(());
            }
            // Choosing the current status accepts the task as it stands without
            // writing anything. A promoted inbox task already left the inbox; a
            // stale task stays stale and returns in a later session.
            let display_ref = entry.item.display_ref.clone();
            let outcome = if entry.reason == TriageReason::Inbox {
                self.set_info(format!("{display_ref} stays {status}"));
                TriageOutcome::Status(status)
            } else {
                // The local review mark keeps a kept task out of triage for one
                // more stale window without touching the task itself.
                let task_id = entry.item.task.id.clone();
                let window = match entry.reason {
                    TriageReason::StaleActive => STALE_ACTIVE_DAYS,
                    _ => STALE_TODO_DAYS,
                };
                let previous = self
                    .store
                    .database()
                    .set_triage_review(task_id.as_str(), Some(now_seconds()), now_seconds())
                    .await?;
                let session = self.triage.as_mut().expect("triage session");
                session.changes.push(TriageChange {
                    index,
                    undo: ChangeUndo::Review { task_id, previous },
                    previous: session.entries[index].outcome,
                });
                self.set_info(format!(
                    "{display_ref} stays {status} · back in triage in {window}d"
                ));
                TriageOutcome::Kept(status)
            };
            let unchanged = self.latest_undo_id();
            return self
                .record_triage_change(index, unchanged, Some(outcome), key)
                .await;
        }
        self.reload_triage_card().await?;
        self.triage.as_mut().expect("triage session").pending = Some(PendingCommand {
            index,
            intent,
            status_before: status,
            undo_before: self.latest_undo_id(),
            key,
        });
        self.execute(action).await?;
        self.settle_triage().await
    }

    /// Runs a custom command against the card's task. The card stays until the
    /// command finishes; it then advances only if the command decided the task.
    async fn run_triage_custom_command(&mut self, handler: CommandHandler) -> Result<()> {
        self.reload_triage_card().await?;
        let undo_before = self.latest_undo_id();
        let session = self.triage.as_mut().expect("triage session");
        session.custom = Some(PendingCommand {
            index: session.cursor,
            intent: TriageIntent::Decide,
            status_before: session.entries[session.cursor].item.task.status,
            undo_before,
            key: None,
        });
        self.execute_command_handler(handler).await?;
        if !self.custom_command_running() {
            self.triage.as_mut().expect("triage session").custom = None;
        }
        Ok(())
    }

    fn custom_command_running(&self) -> bool {
        self.custom_commands.work_pending() || self.pending_terminal_command.is_some()
    }

    /// Reloads the card after a custom command finishes and records the
    /// outcome when the command launched from a card decided its task.
    pub(super) async fn settle_triage_custom_command(&mut self) -> Result<()> {
        if self.triage.is_none() {
            return Ok(());
        }
        let running = self.custom_command_running();
        let session = self.triage.as_mut().expect("triage session");
        let Some(pending) = session.custom.take_if(|_| !running) else {
            return self.reload_triage_card().await;
        };
        let task_id = session.entries[pending.index].item.task.id.clone();
        let item = self.store.load_task_item(&task_id).await?;
        let outcome = decision_outcome(item.as_ref(), &pending);
        if let Some(item) = item {
            self.triage.as_mut().expect("triage session").entries[pending.index].item = item;
        }
        self.record_triage_change(pending.index, pending.undo_before, outcome, None)
            .await
    }

    /// Resolves a command started from a card once any dialog or footer choice
    /// it opened has closed: records the change for undo and advances on a decision.
    pub(super) async fn settle_triage(&mut self) -> Result<()> {
        if self.overlay.is_some() || self.detail.is_active() || self.footer_choice.is_some() {
            return Ok(());
        }
        let Some(session) = self.triage.as_mut() else {
            return Ok(());
        };
        let pending = session.pending.take();
        let latest = self.latest_undo_id();
        let session = self.triage.as_mut().expect("triage session");
        if session.synced_undo == latest {
            return Ok(());
        }
        session.synced_undo = latest.clone();
        let Some(pending) = pending.filter(|pending| pending.undo_before != latest) else {
            return self.reload_triage_card().await;
        };
        let task_id = session.entries[pending.index].item.task.id.clone();
        let item = self.store.load_task_item(&task_id).await?;
        let outcome = decision_outcome(item.as_ref(), &pending);
        if let Some(item) = item {
            self.triage.as_mut().expect("triage session").entries[pending.index].item = item;
        }
        self.record_triage_change(pending.index, pending.undo_before, outcome, pending.key)
            .await
    }

    async fn record_triage_change(
        &mut self,
        index: usize,
        undo_before: Option<String>,
        outcome: Option<TriageOutcome>,
        key: Option<KeyCode>,
    ) -> Result<()> {
        let latest = self.latest_undo_id();
        let session = self.triage.as_mut().expect("triage session");
        session.synced_undo = latest.clone();
        if let Some(entry_id) = latest.filter(|latest| Some(latest) != undo_before.as_ref()) {
            session.changes.push(TriageChange {
                index,
                undo: ChangeUndo::Store(entry_id),
                previous: session.entries[index].outcome,
            });
        }
        if outcome.is_some() {
            session.entries[index].outcome = outcome;
            // A command that finishes after the user moved on records its
            // decision without pulling them away from the current card.
            if session.cursor == index {
                session.cursor = session.next_undecided_after(index);
                session.last_decision = key.map(|key| (key, Instant::now()));
            }
        }
        self.reload_triage_card().await
    }

    /// Undoes the latest change and, when it came from a triage card, returns
    /// to that card with its earlier state.
    async fn undo_triage_change(&mut self) -> Result<()> {
        let session = self.triage.as_mut().expect("triage session");
        if let Some(TriageChange {
            undo: ChangeUndo::Review { .. },
            ..
        }) = session.changes.last()
        {
            let change = session.changes.pop().expect("triage change");
            let ChangeUndo::Review { task_id, previous } = change.undo else {
                unreachable!("review change");
            };
            session.entries[change.index].outcome = change.previous;
            session.cursor = change.index;
            self.store
                .database()
                .set_triage_review(task_id.as_str(), previous, now_seconds())
                .await?;
            self.set_info("undid keep");
            return self.reload_triage_card().await;
        }
        let latest = self.latest_undo_id();
        self.undo_last().await?;
        let after = self.latest_undo_id();
        let session = self.triage.as_mut().expect("triage session");
        session.synced_undo = after;
        if latest.is_some()
            && let Some(TriageChange {
                undo: ChangeUndo::Store(entry_id),
                ..
            }) = session.changes.last()
            && Some(entry_id) == latest.as_ref()
        {
            let change = session.changes.pop().expect("triage change");
            session.entries[change.index].outcome = change.previous;
            session.cursor = change.index;
        }
        self.reload_triage_card().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, project: &str, reason: TriageReason, idle_days: i64) -> TriageEntry {
        let mut item = crate::tui::test_support::task_list_item(title);
        item.task.project_key = project.to_string();
        item.queue.idle_days = Some(idle_days);
        TriageEntry::new(item, reason)
    }

    fn titles(page: &[TriageEntry]) -> Vec<&str> {
        page.iter()
            .map(|entry| entry.item.task.title.as_str())
            .collect()
    }

    #[test]
    fn build_page_interleaves_pools_with_oldest_first() {
        let page = build_page(vec![
            entry("inbox new", "app", TriageReason::Inbox, 2),
            entry("inbox old", "app", TriageReason::Inbox, 40),
            entry("todo", "app", TriageReason::StaleTodo, 90),
            entry("active", "app", TriageReason::StaleActive, 9),
        ]);
        assert_eq!(titles(&page), ["active", "inbox old", "todo", "inbox new"]);
    }

    #[test]
    fn build_page_puts_pressing_work_first_and_caps_the_page() {
        let mut candidates = (0..12)
            .map(|index| entry(&format!("inbox {index}"), "app", TriageReason::Inbox, index))
            .collect::<Vec<_>>();
        let mut urgent = entry("urgent todo", "app", TriageReason::StaleTodo, 31);
        urgent.item.task.priority = TaskPriority::Urgent;
        candidates.push(urgent);
        let page = build_page(candidates);
        assert_eq!(page.len(), PAGE_SIZE);
        assert_eq!(page[0].item.task.title, "urgent todo");
    }
}

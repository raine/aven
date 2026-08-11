use crate::ids::WorkspaceId;
use crate::query::SearchMatchedField;
use crate::tui::authoring::{AddTaskStep, InitialStatusOrigin, PendingTaskAttachmentSummary};
use crate::tui::conflict_flow::ConflictResolutionChoice;
use crate::tui::event::{Action, CommandContext};
use crate::tui::overlay::text_input::LineEdit;
use crate::tui::store::{
    ConflictTarget, EpicContext, TaskOrder, TaskView, TuiDatabaseStats, TuiSyncStatus,
};
use crate::tui::task_selection::TaskSelection;
use crate::tui::text::{char_boundary_at_or_before, normalize_pasted_newlines};
use aven_core::query::{RecurrenceHistoryEntry, RecurrenceHistoryPage};
use aven_core::recurrence::RecurrenceSeriesId;
use chrono::{DateTime, Utc};
use unicode_width::UnicodeWidthStr;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayState {
    Onboarding {
        persist_on_exit: bool,
    },
    Help {
        scroll: u16,
    },
    Detail,
    AttachmentPreview {
        attachment_id: String,
        scroll: u16,
    },
    DetailHelp {
        scroll: u16,
    },
    Search(SearchState),
    Command {
        state: CommandState,
    },
    AddTask(Box<AddTaskState>),
    TextInput(TextInputState),
    MultilineInput(MultilineInputState),
    Picker(PickerState),
    TagCombobox(TagComboboxState),
    HeaderMenu(HeaderMenuState),
    OrderMenu(OrderMenuState),
    Confirm(ConfirmState),
    TextPanel(TextPanelState),
    Changelog(ChangelogState),
    RecurrenceHistory(Box<RecurrenceHistoryState>),
    SyncStatus(Box<TuiSyncStatus>),
    DatabaseStats {
        stats: Box<TuiDatabaseStats>,
        scroll: u16,
    },
    Update(UpdateOverlayState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchResultItem {
    pub(crate) task_id: crate::ids::TaskId,
    pub(crate) display_ref: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project_key: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) created_at: String,
    pub(crate) labels: Vec<String>,
    pub(crate) matched_field: SearchMatchedField,
    pub(crate) snippet: Option<String>,
    pub(crate) score: i64,
    pub(crate) deleted: bool,
    pub(crate) is_epic: bool,
    pub(crate) unavailable_reason: Option<String>,
    pub(crate) create_new: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchIntent {
    Navigate,
    AddDependency {
        selection: crate::tui::task_selection::TaskSelection,
        display_ref: String,
    },
    AddEpicChild {
        epic_id: crate::ids::TaskId,
        display_ref: String,
        project_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchState {
    pub(crate) input: LineEdit,
    pub(crate) results: Vec<SearchResultItem>,
    pub(crate) selected: usize,
    pub(crate) total_matches: usize,
    pub(crate) results_query: Option<String>,
    pub(crate) intent: SearchIntent,
}

impl SearchState {
    pub(crate) fn blank() -> Self {
        Self::for_intent(SearchIntent::Navigate)
    }

    pub(crate) fn for_intent(intent: SearchIntent) -> Self {
        Self {
            input: LineEdit::blank(),
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            results_query: None,
            intent,
        }
    }

    pub(crate) fn current_query(&self) -> String {
        self.input.text.trim().to_string()
    }

    pub(crate) fn clear_results(&mut self) {
        self.results.clear();
        self.selected = 0;
        self.total_matches = 0;
        self.results_query = None;
    }

    pub(crate) fn selected_result(&self) -> Option<&SearchResultItem> {
        self.results.get(self.selected)
    }

    pub(crate) fn results_are_current(&self) -> bool {
        self.results_query.as_deref() == Some(self.input.text.trim())
    }

    pub(crate) fn selected_current_result(&self) -> Option<&SearchResultItem> {
        self.results_are_current()
            .then(|| self.selected_result())
            .flatten()
    }

    pub(crate) fn normalize_selection(&mut self) {
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.results.len() - 1);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayTarget {
    RecurrenceSeries {
        workspace_id: WorkspaceId,
        series_id: RecurrenceSeriesId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandAvailabilityOverride {
    pub(crate) action: Action,
    pub(crate) reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandState {
    pub(crate) input: LineEdit,
    pub(crate) cycle_input: Option<String>,
    pub(crate) cycle_index: usize,
    pub(crate) highlighted: Option<String>,
    pub(crate) context: CommandContext,
    pub(crate) marked_task_count: usize,
    pub(crate) custom_command_marked_task_count: usize,
    pub(crate) target: Option<OverlayTarget>,
    pub(crate) unavailable: Vec<CommandAvailabilityOverride>,
}

impl CommandState {
    pub(crate) fn blank() -> Self {
        Self {
            input: LineEdit::blank(),
            cycle_input: None,
            cycle_index: 0,
            highlighted: None,
            context: CommandContext::Normal,
            marked_task_count: 0,
            custom_command_marked_task_count: 0,
            target: None,
            unavailable: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(input: LineEdit) -> Self {
        Self {
            input,
            cycle_input: None,
            cycle_index: 0,
            highlighted: None,
            context: CommandContext::Normal,
            marked_task_count: 0,
            custom_command_marked_task_count: 0,
            target: None,
            unavailable: Vec::new(),
        }
    }

    pub(crate) fn reset_cycle(&mut self) {
        self.cycle_input = None;
        self.cycle_index = 0;
        self.highlighted = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangelogState {
    pub(crate) markdown: String,
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelState {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
}

pub(crate) const RECURRENCE_HISTORY_PAGE_SIZE: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RecurrenceHistoryEntryKey {
    Slot(String),
    PauseStartedAt(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecurrenceHistoryAction {
    OpenTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecurrenceHistoryState {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) series_id: RecurrenceSeriesId,
    pub(crate) as_of: DateTime<Utc>,
    pub(crate) page: RecurrenceHistoryPage,
    pub(crate) selected: Option<usize>,
}

impl RecurrenceHistoryState {
    pub(crate) fn new(
        workspace_id: WorkspaceId,
        series_id: RecurrenceSeriesId,
        as_of: DateTime<Utc>,
        page: RecurrenceHistoryPage,
    ) -> Self {
        let selected = (!page.items.is_empty()).then_some(0);
        Self {
            workspace_id,
            series_id,
            as_of,
            page,
            selected,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub(crate) fn selected_entry(&self) -> Option<&RecurrenceHistoryEntry> {
        self.selected.and_then(|index| self.page.items.get(index))
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if self.page.items.is_empty() {
            self.selected = None;
            return;
        }
        let Some(current) = self.selected else {
            self.selected = Some(0);
            return;
        };
        let last = self.page.items.len().saturating_sub(1);
        self.selected = Some(current.saturating_add_signed(delta).min(last));
    }

    pub(crate) fn replace_page(
        &mut self,
        page: RecurrenceHistoryPage,
        preferred: Option<RecurrenceHistoryEntryKey>,
        fallback_index: usize,
    ) {
        self.page = page;
        self.selected = preferred
            .and_then(|key| {
                self.page
                    .items
                    .iter()
                    .position(|entry| recurrence_history_entry_key(entry) == key)
            })
            .or_else(|| {
                (!self.page.items.is_empty())
                    .then(|| fallback_index.min(self.page.items.len().saturating_sub(1)))
            });
    }
}

fn recurrence_history_entry_key(entry: &RecurrenceHistoryEntry) -> RecurrenceHistoryEntryKey {
    match (entry.slot_on.as_ref(), entry.interval_started_at.as_ref()) {
        (Some(slot), None) => RecurrenceHistoryEntryKey::Slot(slot.clone()),
        (None, Some(started_at)) => RecurrenceHistoryEntryKey::PauseStartedAt(started_at.clone()),
        _ => panic!("history entry must identify one slot or pause interval"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateNotesState {
    Loading,
    Ready(String),
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateActionFocus {
    Later,
    Primary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateOverlayState {
    Checking,
    Available {
        plan: crate::update::InstallPlan,
        notes: UpdateNotesState,
        scroll: u16,
        focus: UpdateActionFocus,
        cached: bool,
    },
    Progress {
        version: String,
        phase: crate::update::UpdatePhase,
        cancelling: bool,
    },
    Current {
        version: String,
        cached: bool,
    },
    Success {
        version: String,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

pub(crate) const ORDER_MENU_WIDTH: u16 = 20;
pub(crate) const ORDER_MENU_HEIGHT: u16 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderMenuState {
    pub(crate) kind: HeaderMenuKind,
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) selected: usize,
    pub(crate) items: Vec<HeaderMenuItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderMenuKind {
    Workspace,
    Scope,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeaderMenuAction {
    Workspace(String),
    WorkspaceScope,
    ProjectScope(String),
    View(TaskView),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderMenuItem {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) selected: bool,
    pub(crate) action: HeaderMenuAction,
}

impl HeaderMenuState {
    pub(crate) fn area(&self, terminal_width: u16, terminal_height: u16) -> ratatui::layout::Rect {
        let width = self.width().min(terminal_width);
        let height = (self.items.len() as u16)
            .saturating_add(2)
            .min(terminal_height);
        let x = self.column.min(terminal_width.saturating_sub(width));
        let y = self
            .row
            .saturating_add(1)
            .min(terminal_height.saturating_sub(height));
        ratatui::layout::Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn selected_action(&self) -> Option<HeaderMenuAction> {
        self.items
            .get(self.selected)
            .map(|item| item.action.clone())
    }

    fn width(&self) -> u16 {
        let title_width = self.title().width() as u16;
        let item_width = self
            .items
            .iter()
            .map(|item| item.line_width())
            .max()
            .unwrap_or(0);
        title_width.max(item_width).saturating_add(4).max(16)
    }

    fn title(&self) -> &'static str {
        match self.kind {
            HeaderMenuKind::Workspace => "workspace",
            HeaderMenuKind::Scope => "scope",
            HeaderMenuKind::View => "view",
        }
    }
}

impl HeaderMenuItem {
    fn line_width(&self) -> u16 {
        "▸ ".width() as u16
            + format!("{:<2}", self.key).width() as u16
            + " ".width() as u16
            + self.label.width() as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OrderMenuState {
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) selected: TaskOrder,
}

impl OrderMenuState {
    pub(crate) fn area(&self, terminal_width: u16, terminal_height: u16) -> ratatui::layout::Rect {
        let width = ORDER_MENU_WIDTH.min(terminal_width);
        let height = ORDER_MENU_HEIGHT.min(terminal_height);
        let x = self.column.min(terminal_width.saturating_sub(width));
        let y = self
            .row
            .saturating_add(1)
            .min(terminal_height.saturating_sub(height));
        ratatui::layout::Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl TextPanelState {
    pub(crate) fn new(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            lines,
            scroll: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TextIntent {
    AddProject,
    AddProjectPath {
        project: String,
    },
    AddLabel,
    RenameLabel {
        label: String,
    },
    ConfirmDeleteLabel {
        label: String,
        task_count: usize,
        series_count: usize,
    },
    AddWorkspace,
    RenameWorkspace {
        workspace: String,
    },
    RenameProject {
        project: String,
    },
    ConfirmDeleteProject {
        project: String,
    },
    EditTitle {
        selection: TaskSelection,
    },
    EditAvailability {
        selection: TaskSelection,
        mixed: bool,
    },
    EditDue {
        selection: TaskSelection,
        mixed: bool,
    },
    SaveAttachment {
        attachment_id: String,
        filename: String,
        scroll: u16,
    },
    ResolveConflictManually {
        target: ConflictTarget,
    },
}

impl TextIntent {
    pub(crate) fn is_date_edit(&self) -> bool {
        matches!(self, Self::EditAvailability { .. } | Self::EditDue { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MultilineIntent {
    AddTaskDescription,
    AddTaskNatural,
    AddNote {
        task_id: crate::ids::TaskId,
        display_ref: String,
    },
    EditNote {
        task_id: crate::ids::TaskId,
        display_ref: String,
        note_id: String,
    },
    EditDescription {
        selection: TaskSelection,
    },
    ResolveConflictManually {
        target: ConflictTarget,
    },
}

impl MultilineIntent {
    pub(crate) fn supports_external_editor(&self) -> bool {
        matches!(self, Self::EditDescription { .. } | Self::EditNote { .. })
    }

    pub(crate) fn is_description_edit(&self) -> bool {
        matches!(self, Self::EditDescription { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerIntent {
    AddTaskProject,
    AddTaskStatus,
    AddTaskPriority,
    MoveToColumn {
        selection: TaskSelection,
    },
    EditProject {
        selection: TaskSelection,
        mixed: bool,
    },
    EditPriority {
        selection: TaskSelection,
        mixed: bool,
    },
    EditEpic {
        selection: TaskSelection,
        mixed: bool,
    },
    FilterLabel,
    FilterPriority,
    ScopeProject,
    RenameProject,
    DeleteProject,
    AddProjectPath,
    RemoveProjectPath,
    RemoveProjectPathValue {
        project: String,
    },
    BrowseLabels,
    LabelActions {
        label: String,
    },
    RenameLabel,
    DeleteLabel,
    SwitchWorkspace,
    RenameWorkspace,
    PickConflictVariant {
        choice: ConflictResolutionChoice,
        targets: Vec<ConflictTarget>,
    },
    PickConflictManual {
        targets: Vec<ConflictTarget>,
    },
    ResolveConflictManually {
        target: ConflictTarget,
    },
    RemoveDependency {
        selection: crate::tui::task_selection::TaskSelection,
    },
    RecurrenceActions {
        target: OverlayTarget,
    },
    StopRecurrence {
        target: OverlayTarget,
    },
}

impl PickerIntent {
    pub(crate) fn filter_escape_cancels(&self) -> bool {
        matches!(self, Self::ScopeProject | Self::SwitchWorkspace)
    }

    pub(crate) fn initial_mode(&self) -> PickerMode {
        match self {
            Self::AddTaskProject
            | Self::EditProject { .. }
            | Self::ScopeProject
            | Self::RenameProject
            | Self::DeleteProject
            | Self::AddProjectPath
            | Self::RemoveProjectPath
            | Self::RemoveProjectPathValue { .. }
            | Self::BrowseLabels
            | Self::RenameLabel
            | Self::DeleteLabel
            | Self::SwitchWorkspace
            | Self::RenameWorkspace => PickerMode::Filter,
            _ => PickerMode::Navigate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TagComboboxIntent {
    AddTaskLabels,
    EditLabels { selection: TaskSelection },
    EditLabelsMulti { selection: TaskSelection },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfirmIntent {
    ResolveConflict {
        target: ConflictTarget,
        value: String,
    },
    InitializeConfig {
        path: std::path::PathBuf,
    },
    DeleteProject {
        project: String,
    },
    RemoveProjectPath {
        project: String,
        path: String,
    },
    DeleteLabel {
        label: String,
    },
    DeleteTasks {
        selection: TaskSelection,
    },
    DeleteNote {
        task_id: crate::ids::TaskId,
        note_id: String,
    },
    DeleteFocusedTask {
        selection: TaskSelection,
    },
    UnlinkDependency {
        selection: TaskSelection,
        depends_on_task_id: crate::ids::TaskId,
    },
    UnlinkEpicChild {
        epic_id: crate::ids::TaskId,
        child_id: crate::ids::TaskId,
    },
    DeleteAttachment {
        attachment_id: String,
    },
    PromoteTaskForChild {
        epic: EpicContext,
    },
    CreateTaskGist {
        task_id: crate::ids::TaskId,
    },
    ClearAvailability {
        selection: TaskSelection,
    },
    ClearDue {
        selection: TaskSelection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScheduleEditorMode {
    Once,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScheduleEditorField {
    Mode,
    Available,
    Due,
    Repeat,
    Time,
    DuePolicy,
    Starts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleEditorState {
    pub(crate) mode: ScheduleEditorMode,
    pub(crate) focus: ScheduleEditorField,
    pub(crate) available_at: LineEdit,
    pub(crate) due_on: LineEdit,
    pub(crate) repeat_rule: LineEdit,
    pub(crate) repeat_at: LineEdit,
    pub(crate) repeat_due: String,
    pub(crate) repeat_start_on: LineEdit,
    pub(crate) time_zone: String,
    pub(crate) template_locked: bool,
    pub(crate) preview: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) validation_requested: bool,
}

impl ScheduleEditorState {
    pub(crate) fn fields(&self) -> &'static [ScheduleEditorField] {
        match self.mode {
            ScheduleEditorMode::Once => &[
                ScheduleEditorField::Mode,
                ScheduleEditorField::Available,
                ScheduleEditorField::Due,
            ],
            ScheduleEditorMode::Repeat if self.template_locked => &[
                ScheduleEditorField::Mode,
                ScheduleEditorField::Time,
                ScheduleEditorField::DuePolicy,
            ],
            ScheduleEditorMode::Repeat => &[
                ScheduleEditorField::Mode,
                ScheduleEditorField::Repeat,
                ScheduleEditorField::Time,
                ScheduleEditorField::DuePolicy,
                ScheduleEditorField::Starts,
            ],
        }
    }

    pub(crate) fn focus_next(&mut self, reverse: bool) {
        let fields = self.fields();
        let index = fields
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        self.focus = if reverse {
            fields[index.checked_sub(1).unwrap_or(fields.len() - 1)]
        } else {
            fields[(index + 1) % fields.len()]
        };
    }

    pub(crate) fn cycle_mode(&mut self, _reverse: bool) {
        if self.template_locked {
            return;
        }
        self.mode = match self.mode {
            ScheduleEditorMode::Once => ScheduleEditorMode::Repeat,
            ScheduleEditorMode::Repeat => ScheduleEditorMode::Once,
        };
        self.focus = ScheduleEditorField::Mode;
        self.validation_requested = false;
        self.refresh();
    }

    pub(crate) fn refresh(&mut self) {
        self.preview.clear();
        let error = match self.mode {
            ScheduleEditorMode::Once => {
                let available = if self.available_at.text.trim().is_empty() {
                    Ok(String::new())
                } else {
                    crate::time_input::parse_available_at_input(&self.available_at.text)
                };
                available
                    .and_then(|_| {
                        if self.due_on.text.trim().is_empty() {
                            Ok(String::new())
                        } else {
                            crate::time_input::parse_due_on_input(&self.due_on.text)
                        }
                    })
                    .err()
                    .map(|error| format!("{error:#}"))
            }
            ScheduleEditorMode::Repeat => {
                let repeat_at = Some(self.repeat_at.text.trim()).filter(|value| !value.is_empty());
                let starts_on =
                    Some(self.repeat_start_on.text.trim()).filter(|value| !value.is_empty());
                match crate::recurrence_input::canonical_rule_input(&self.repeat_rule.text)
                    .and_then(|rule| {
                        let Some(rule) = rule else {
                            anyhow::bail!(crate::recurrence_input::rule_guidance());
                        };
                        crate::commands::recurrence_schedule(
                            &rule,
                            repeat_at,
                            Some(&self.repeat_due),
                            Some(self.time_zone.trim()).filter(|value| !value.is_empty()),
                            starts_on,
                        )
                    }) {
                    Ok(schedule) => {
                        let zone = schedule
                            .timezone
                            .as_str()
                            .parse::<chrono_tz::Tz>()
                            .expect("validated recurrence time zone parses");
                        let from = schedule
                            .start_on
                            .max(Utc::now().with_timezone(&zone).date_naive());
                        self.preview = schedule
                            .slots_on_or_after(from)
                            .take(3)
                            .map(|date| date.format("%a %b %-d").to_string())
                            .collect();
                        None
                    }
                    Err(error) => Some(format!("{error:#}")),
                }
            }
        };
        self.error = self.validation_requested.then_some(error).flatten();
    }

    pub(crate) fn validate_current_field(&mut self) {
        if !matches!(
            self.focus,
            ScheduleEditorField::Mode | ScheduleEditorField::DuePolicy
        ) {
            self.validate();
        }
    }

    pub(crate) fn validate(&mut self) {
        self.validation_requested = true;
        self.refresh();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddTaskMode {
    Compose,
    Schedule(ScheduleEditorState),
    Picker {
        field: AddTaskStep,
        state: PickerState,
    },
    Labels(TagComboboxState),
    Help {
        scroll: u16,
    },
    ConfirmDiscard,
}

impl AddTaskMode {
    pub(crate) fn expands_composer(&self) -> bool {
        match self {
            Self::Help { .. } => true,
            Self::Compose
            | Self::Schedule(_)
            | Self::Picker { .. }
            | Self::Labels(_)
            | Self::ConfirmDiscard => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskState {
    pub(crate) title: LineEdit,
    pub(crate) description: MultilineInputState,
    pub(crate) focus: AddTaskStep,
    pub(crate) project: String,
    pub(crate) inferred_project: Option<String>,
    pub(crate) selected_project: Option<String>,
    pub(crate) initial_project: Option<String>,
    pub(crate) status: String,
    pub(crate) status_origin: InitialStatusOrigin,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) is_epic: bool,
    pub(crate) create_more: bool,
    pub(crate) create_more_available: bool,
    pub(crate) available_at: LineEdit,
    pub(crate) due_on: LineEdit,
    pub(crate) schedule_input: LineEdit,
    pub(crate) schedule_error: Option<String>,
    pub(crate) schedule_validation_requested: bool,
    pub(crate) attachments: Vec<PendingTaskAttachmentSummary>,
    pub(crate) selected_attachment: usize,
    pub(crate) recurrence_series_id: Option<aven_core::recurrence::RecurrenceSeriesId>,
    pub(crate) template_schedule: Option<aven_core::recurrence::RecurrenceSchedule>,
    pub(crate) repeat_rule: LineEdit,
    pub(crate) repeat_at: LineEdit,
    pub(crate) repeat_due: String,
    pub(crate) time_zone: String,
    pub(crate) repeat_start_on: LineEdit,
    pub(crate) schedule_expanded: bool,
    pub(crate) recurrence_preview: Vec<String>,
    pub(crate) recurrence_error: Option<String>,
    pub(crate) mode: AddTaskMode,
    pub(crate) title_error: bool,
}

impl AddTaskState {
    pub(crate) fn schedule_editor(&self, focus: ScheduleEditorField) -> ScheduleEditorState {
        let mode = if self.recurrence_enabled() {
            ScheduleEditorMode::Repeat
        } else {
            ScheduleEditorMode::Once
        };
        ScheduleEditorState {
            mode,
            focus,
            available_at: self.available_at.clone(),
            due_on: self.due_on.clone(),
            repeat_rule: self.repeat_rule.clone(),
            repeat_at: self.repeat_at.clone(),
            repeat_due: self.repeat_due.clone(),
            repeat_start_on: self.repeat_start_on.clone(),
            time_zone: self.time_zone.clone(),
            template_locked: self.template_schedule.is_some(),
            preview: self.recurrence_preview.clone(),
            validation_requested: self.recurrence_error.is_some(),
            error: self.recurrence_error.clone(),
        }
    }

    pub(crate) fn apply_schedule_input(&mut self) {
        match crate::schedule_input::parse_schedule_input(&self.schedule_input.text) {
            Ok(crate::schedule_input::ParsedScheduleInput::None) => {
                self.available_at = LineEdit::blank();
                self.due_on = LineEdit::blank();
                self.repeat_rule = LineEdit::blank();
                self.schedule_error = None;
            }
            Ok(crate::schedule_input::ParsedScheduleInput::Once {
                available_at,
                due_on,
            }) => {
                self.available_at = LineEdit::new(available_at);
                self.due_on = LineEdit::new(due_on);
                self.repeat_rule = LineEdit::blank();
                self.schedule_error = None;
            }
            Ok(crate::schedule_input::ParsedScheduleInput::Recurring {
                rule,
                available_time,
                due_policy,
                starts_on,
            }) if self.template_schedule.is_none() => {
                self.available_at = LineEdit::blank();
                self.due_on = LineEdit::blank();
                self.repeat_rule = LineEdit::new(rule);
                self.repeat_at = LineEdit::new(available_time);
                self.repeat_due = due_policy;
                if !starts_on.is_empty() {
                    self.repeat_start_on = LineEdit::new(starts_on);
                }
                self.schedule_error = None;
            }
            Ok(crate::schedule_input::ParsedScheduleInput::Recurring { .. }) => {
                self.schedule_error =
                    Some("The repeat rule and start date are fixed for this template".to_string());
            }
            Err(error) => self.schedule_error = Some(format!("{error:#}")),
        }
        self.refresh_repeat_status();
        self.refresh_recurrence_preview();
    }

    pub(crate) fn canonicalize_schedule_input(&mut self) {
        if self.schedule_error.is_none() {
            self.schedule_input = LineEdit::new(crate::schedule_input::format_schedule_input(
                &self.available_at.text,
                &self.due_on.text,
                &self.repeat_rule.text,
                &self.repeat_at.text,
                &self.repeat_due,
                &self.repeat_start_on.text,
            ));
        }
    }

    pub(crate) fn apply_schedule_editor(&mut self, editor: ScheduleEditorState) {
        match editor.mode {
            ScheduleEditorMode::Once if !editor.template_locked => {
                self.available_at = editor.available_at;
                self.due_on = editor.due_on;
                self.repeat_rule = LineEdit::blank();
            }
            ScheduleEditorMode::Repeat => {
                self.available_at = LineEdit::blank();
                self.due_on = LineEdit::blank();
                self.repeat_rule = editor.repeat_rule;
                self.repeat_at = editor.repeat_at;
                self.repeat_due = editor.repeat_due;
                self.repeat_start_on = editor.repeat_start_on;
                self.time_zone = editor.time_zone;
            }
            _ => {}
        }
        self.schedule_input = LineEdit::new(crate::schedule_input::format_schedule_input(
            &self.available_at.text,
            &self.due_on.text,
            &self.repeat_rule.text,
            &self.repeat_at.text,
            &self.repeat_due,
            &self.repeat_start_on.text,
        ));
        self.schedule_error = None;
        self.refresh_repeat_status();
        self.refresh_recurrence_preview();
    }

    pub(crate) fn is_populated(&self) -> bool {
        !self.title.text.trim().is_empty()
            || self
                .description
                .lines
                .iter()
                .any(|line| !line.trim().is_empty())
            || self.selected_project != self.initial_project
            || self.status != "inbox"
            || self.priority != "none"
            || !self.labels.is_empty()
            || self.is_epic
            || !self.available_at.text.trim().is_empty()
            || !self.due_on.text.trim().is_empty()
            || !self.attachments.is_empty()
            || self.recurrence_series_id.is_some()
            || !matches!(self.repeat_rule.text.trim(), "" | "none")
    }

    pub(crate) fn focus_next(&mut self, reverse: bool) {
        for _ in 0..AddTaskStep::ALL.len() {
            self.focus = self.focus.next(reverse);
            let has_visible_image_step =
                self.focus != AddTaskStep::Images || !self.attachments.is_empty();
            if has_visible_image_step && self.is_step_editable(self.focus) {
                break;
            }
        }
    }

    pub(crate) fn focus_metadata_next(&mut self, reverse: bool) {
        for _ in 0..AddTaskStep::ALL.len() {
            self.focus = self.focus.metadata_next(reverse);
            if self.is_step_editable(self.focus) {
                break;
            }
        }
    }

    pub(crate) fn is_step_editable(&self, step: AddTaskStep) -> bool {
        if step == AddTaskStep::Schedule {
            return true;
        }
        if step.is_schedule_field() {
            return false;
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn set_repeat_rule(&mut self, repeat_rule: String) {
        self.repeat_rule = LineEdit::new(repeat_rule);
        self.refresh_repeat_status();
    }

    pub(crate) fn refresh_repeat_status(&mut self) {
        let enabled = self.recurrence_valid();
        if enabled {
            self.create_more = false;
        }
        match (enabled, self.status_origin) {
            (true, InitialStatusOrigin::UntouchedDefault) if self.status == "inbox" => {
                self.status = "todo".to_string();
                self.status_origin = InitialStatusOrigin::RecurrenceDefault;
            }
            (false, InitialStatusOrigin::RecurrenceDefault) => {
                self.status = "inbox".to_string();
                self.status_origin = InitialStatusOrigin::UntouchedDefault;
            }
            _ => {}
        }
    }

    pub(crate) fn recurrence_enabled(&self) -> bool {
        self.template_schedule.is_some() || !matches!(self.repeat_rule.text.trim(), "" | "none")
    }

    pub(crate) fn recurrence_valid(&self) -> bool {
        self.template_schedule.is_some()
            || self
                .recurrence_rule_input()
                .is_ok_and(|rule| rule.is_some())
    }

    pub(crate) fn recurrence_rule_input(&self) -> anyhow::Result<Option<String>> {
        crate::recurrence_input::canonical_rule_input(&self.repeat_rule.text)
    }

    pub(crate) fn recurrence_schedule(
        &self,
    ) -> anyhow::Result<Option<aven_core::recurrence::RecurrenceSchedule>> {
        if !self.recurrence_enabled() {
            return Ok(None);
        }
        let repeat_at = Some(self.repeat_at.text.trim()).filter(|value| !value.is_empty());
        let start_on = Some(self.repeat_start_on.text.trim()).filter(|value| !value.is_empty());
        if let Some(template) = self.template_schedule.as_ref() {
            let mutable = crate::commands::recurrence_schedule(
                "daily",
                repeat_at,
                Some(&self.repeat_due),
                Some(template.timezone.as_str()),
                Some(&template.start_on.to_string()),
            )?;
            return Ok(Some(aven_core::recurrence::RecurrenceSchedule::new(
                template.rule,
                template.timezone.clone(),
                template.start_on,
                mutable.available_local_time,
                mutable.due_policy,
            )));
        }
        let Some(rule) = self.recurrence_rule_input()? else {
            return Ok(None);
        };
        crate::commands::recurrence_schedule(
            &rule,
            repeat_at,
            Some(&self.repeat_due),
            Some(self.time_zone.trim()).filter(|value| !value.is_empty()),
            start_on,
        )
        .map(Some)
    }

    pub(crate) fn refresh_recurrence_preview(&mut self) {
        self.refresh_recurrence_preview_at(Utc::now());
    }

    pub(crate) fn refresh_recurrence_preview_at(&mut self, now: DateTime<Utc>) {
        self.recurrence_preview.clear();
        self.recurrence_error = None;
        let Some(schedule) = (match self.recurrence_schedule() {
            Ok(schedule) => schedule,
            Err(error) => {
                self.recurrence_error = Some(format!("{error:#}"));
                return;
            }
        }) else {
            return;
        };
        if schedule.rule.frequency() == aven_core::recurrence::RecurrenceFrequency::Weekly
            && schedule.rule.interval() > 5200
        {
            self.recurrence_error =
                Some("preview unavailable for intervals above 5200 weeks".to_string());
            return;
        }
        let zone = schedule
            .timezone
            .as_str()
            .parse::<chrono_tz::Tz>()
            .expect("core-validated time zone parses with chrono-tz");
        let from = schedule.start_on.max(now.with_timezone(&zone).date_naive());
        self.recurrence_preview = schedule
            .slots_on_or_after(from)
            .take(3)
            .map(|date| date.format("%a %b %-d").to_string())
            .collect();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextInputState {
    pub(crate) intent: TextIntent,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) input: LineEdit,
}

impl TextInputState {
    pub(crate) fn new(
        intent: TextIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        input: String,
    ) -> Self {
        Self {
            intent,
            title: title.into(),
            prompt: prompt.into(),
            input: LineEdit::new(input),
        }
    }

    pub(crate) fn blank(
        intent: TextIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::new(intent, title, prompt, String::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MultilineInputMode {
    Compose,
    ConfirmDiscard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultilineInputState {
    pub(crate) intent: MultilineIntent,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) mode: MultilineInputMode,
    baseline: Vec<String>,
}

impl MultilineInputState {
    pub(crate) fn is_dirty(&self) -> bool {
        self.lines != self.baseline
    }

    pub(crate) fn should_confirm_discard(&self) -> bool {
        self.is_dirty()
            && matches!(
                self.intent,
                MultilineIntent::AddNote { .. }
                    | MultilineIntent::EditNote { .. }
                    | MultilineIntent::EditDescription { .. }
                    | MultilineIntent::ResolveConflictManually { .. }
            )
    }

    pub(crate) fn baseline_value(&self) -> String {
        self.baseline.join("\n")
    }

    pub(crate) fn insert_paste(&mut self, text: &str) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let row = self.row.min(self.lines.len() - 1);
        let column = char_boundary_at_or_before(&self.lines[row], self.column);
        self.row = row;
        self.column = column;

        let text = normalize_pasted_newlines(text);
        let mut pasted_lines = text.split('\n');
        let first = pasted_lines.next().unwrap_or_default();
        let rest = self.lines[row].split_off(column);
        self.lines[row].push_str(first);

        let mut insert_at = row;
        for line in pasted_lines {
            insert_at += 1;
            self.lines.insert(insert_at, line.to_string());
        }
        self.lines[insert_at].push_str(&rest);
        self.row = insert_at;
        self.column = self.lines[insert_at].len().saturating_sub(rest.len());
    }

    pub(crate) fn blank(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::from_value(intent, title, prompt, String::new())
    }

    pub(crate) fn from_value(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
    ) -> Self {
        Self::from_value_with_baseline(intent, title, prompt, value.clone(), value)
    }

    pub(crate) fn from_value_with_baseline(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
        baseline: String,
    ) -> Self {
        let lines = value.split('\n').map(str::to_string).collect::<Vec<_>>();
        let baseline = baseline.split('\n').map(str::to_string).collect::<Vec<_>>();
        let row = lines.len() - 1;
        let column = lines[row].len();
        Self {
            intent,
            title: title.into(),
            prompt: prompt.into(),
            lines,
            row,
            column,
            mode: MultilineInputMode::Compose,
            baseline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerMode {
    Navigate,
    Filter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerState {
    pub(crate) intent: PickerIntent,
    pub(crate) title: String,
    pub(crate) filter: LineEdit,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
    pub(crate) multi: bool,
    pub(crate) mode: PickerMode,
}

impl PickerState {
    pub(crate) fn new(
        intent: PickerIntent,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        multi: bool,
    ) -> Self {
        let selected = Self::selected_index(&items);
        let mode = intent.initial_mode();
        Self {
            intent,
            title: title.into(),
            filter: LineEdit::blank(),
            items,
            selected,
            scroll: 0,
            multi,
            mode,
        }
    }

    fn selected_index(items: &[PickerItem]) -> usize {
        items.iter().position(|item| item.selected).unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagComboboxState {
    pub(crate) intent: TagComboboxIntent,
    pub(crate) title: String,
    pub(crate) input: LineEdit,
    pub(crate) options: Vec<String>,
    pub(crate) selected: Vec<String>,
    pub(crate) partial: Vec<String>,
    pub(crate) highlighted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerItem {
    pub(crate) label: String,
    pub(crate) value: String,
    pub(crate) selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmState {
    pub(crate) intent: ConfirmIntent,
    pub(crate) title: String,
    pub(crate) prompt: String,
}

impl ConfirmState {
    pub(crate) fn new(
        intent: ConfirmIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            intent,
            title: title.into(),
            prompt: prompt.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlaySubmit {
    AddTask(Box<AddTaskState>),
    CreateAddTaskProject {
        state: Box<AddTaskState>,
        name: String,
    },
    Text {
        intent: TextIntent,
        value: String,
    },
    ClearDate {
        intent: TextIntent,
    },
    Multiline {
        intent: MultilineIntent,
        value: String,
    },
    Picker {
        intent: PickerIntent,
        values: Vec<String>,
        partial_values: Vec<String>,
    },
    TagCombobox {
        intent: TagComboboxIntent,
        values: Vec<String>,
        partial_values: Vec<String>,
    },
    HeaderMenu {
        action: HeaderMenuAction,
    },
    Order {
        order: TaskOrder,
    },
    Confirm {
        intent: ConfirmIntent,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayOutcome {
    None(OverlayState),
    Cancelled,
    Submitted(OverlaySubmit),
}

impl OverlayState {
    pub(crate) fn text_input(
        intent: TextIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        input: String,
    ) -> Self {
        Self::TextInput(TextInputState::new(intent, title, prompt, input))
    }

    pub(crate) fn blank_text_input(
        intent: TextIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::TextInput(TextInputState::blank(intent, title, prompt))
    }

    pub(crate) fn multiline_input(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
    ) -> Self {
        Self::MultilineInput(MultilineInputState::from_value(
            intent, title, prompt, value,
        ))
    }

    pub(crate) fn multiline_input_with_baseline(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
        baseline: String,
    ) -> Self {
        Self::MultilineInput(MultilineInputState::from_value_with_baseline(
            intent, title, prompt, value, baseline,
        ))
    }

    pub(crate) fn blank_multiline_input(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::MultilineInput(MultilineInputState::blank(intent, title, prompt))
    }

    pub(crate) fn picker(
        intent: PickerIntent,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        multi: bool,
    ) -> Self {
        Self::Picker(PickerState::new(intent, title, items, multi))
    }

    pub(crate) fn tag_combobox(
        intent: TagComboboxIntent,
        title: impl Into<String>,
        options: Vec<String>,
        selected: Vec<String>,
    ) -> Self {
        let highlighted = options
            .iter()
            .position(|label| selected.contains(label))
            .unwrap_or(0);
        Self::TagCombobox(TagComboboxState {
            intent,
            title: title.into(),
            input: LineEdit::blank(),
            options,
            selected,
            partial: Vec::new(),
            highlighted,
        })
    }

    pub(crate) fn partial_tag_combobox(
        intent: TagComboboxIntent,
        title: impl Into<String>,
        options: Vec<String>,
        selected: Vec<String>,
        partial: Vec<String>,
    ) -> Self {
        let highlighted = options
            .iter()
            .position(|label| selected.contains(label) || partial.contains(label))
            .unwrap_or(0);
        Self::TagCombobox(TagComboboxState {
            intent,
            title: title.into(),
            input: LineEdit::blank(),
            options,
            selected,
            partial,
            highlighted,
        })
    }

    pub(crate) fn confirm(
        intent: ConfirmIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::Confirm(ConfirmState::new(intent, title, prompt))
    }

    pub(crate) fn captures_input(&self) -> bool {
        true
    }
    pub(crate) fn header_menu(
        kind: HeaderMenuKind,
        column: u16,
        row: u16,
        items: Vec<HeaderMenuItem>,
    ) -> Self {
        let selected = items.iter().position(|item| item.selected).unwrap_or(0);
        Self::HeaderMenu(HeaderMenuState {
            kind,
            column,
            row,
            selected,
            items,
        })
    }

    pub(crate) fn order_menu(column: u16, row: u16, selected: TaskOrder) -> Self {
        Self::OrderMenu(OrderMenuState {
            column,
            row,
            selected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aven_core::query::RecurrenceHistoryKind;

    #[test]
    fn picker_builder_uses_intent_mode_and_first_selected_item() {
        let state = PickerState::new(
            PickerIntent::ScopeProject,
            "Project",
            vec![
                PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Two".to_string(),
                    value: "two".to_string(),
                    selected: true,
                },
            ],
            false,
        );

        assert_eq!(state.selected, 1);
        assert_eq!(state.mode, PickerMode::Filter);
        assert_eq!(state.intent, PickerIntent::ScopeProject);
    }

    #[test]
    fn payload_bearing_intents_keep_flow_data_inside_state() {
        let search = SearchState::for_intent(SearchIntent::AddEpicChild {
            epic_id: crate::test_support::task_id("epic-1"),
            display_ref: "APP-1234".to_string(),
            project_key: "app".to_string(),
        });
        assert!(matches!(
            search.intent,
            SearchIntent::AddEpicChild {
                ref display_ref,
                ref project_key,
                    ..
            } if display_ref == "APP-1234" && project_key == "app"
        ));

        let confirm = ConfirmState::new(
            ConfirmIntent::DeleteAttachment {
                attachment_id: "attachment-1".to_string(),
            },
            "Delete attachment",
            "Delete attachment?",
        );
        assert!(matches!(
            confirm.intent,
            ConfirmIntent::DeleteAttachment {
                ref attachment_id,
                } if attachment_id == "attachment-1"
        ));
    }

    #[test]
    fn input_builders_store_intents_with_editor_state() {
        let OverlayState::TextInput(text) =
            OverlayState::blank_text_input(TextIntent::AddProject, "Add project", "project name:")
        else {
            panic!("expected text input");
        };
        assert_eq!(text.intent, TextIntent::AddProject);

        let OverlayState::MultilineInput(multiline) = OverlayState::multiline_input(
            MultilineIntent::AddTaskNatural,
            "Add task",
            "",
            "one\ntwo".to_string(),
        ) else {
            panic!("expected multiline input");
        };
        assert_eq!(multiline.intent, MultilineIntent::AddTaskNatural);
        assert_eq!(multiline.lines, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn recurrence_history_selection_uses_resident_indexes_and_reload_identity() {
        let workspace_id = WorkspaceId::new();
        let series_id = RecurrenceSeriesId::new();
        let page = RecurrenceHistoryPage {
            series_ref: "RCR-TEST".to_string(),
            items: vec![history_entry("2026-07-22"), history_entry("2026-07-21")],
            offset: 0,
            limit: 10,
            total: 2,
            has_more: false,
        };
        let mut state = RecurrenceHistoryState::new(workspace_id, series_id, Utc::now(), page);

        assert_eq!(state.selected_index(), Some(0));
        state.move_selection(1);
        assert_eq!(state.selected_index(), Some(1));
        let selected = state.selected_entry().map(recurrence_history_entry_key);
        state.replace_page(
            RecurrenceHistoryPage {
                series_ref: "RCR-TEST".to_string(),
                items: vec![history_entry("2026-07-21"), history_entry("2026-07-20")],
                offset: 1,
                limit: 10,
                total: 2,
                has_more: false,
            },
            selected,
            0,
        );
        assert_eq!(state.selected_index(), Some(0));
    }

    fn history_entry(slot_on: &str) -> RecurrenceHistoryEntry {
        RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Missed,
            slot_on: Some(slot_on.to_string()),
            interval_started_at: None,
            interval_ended_at: None,
            task_id: None,
            task_ref: None,
            openable: false,
            archived_projection: false,
            resolved_at: None,
        }
    }
}

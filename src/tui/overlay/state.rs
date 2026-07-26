use crate::query::SearchMatchedField;
use crate::tui::authoring::{AddTaskStep, PendingTaskAttachmentSummary};
use crate::tui::conflict_flow::ConflictResolutionChoice;
use crate::tui::overlay::text_input::LineEdit;
use crate::tui::store::{ConflictTarget, TaskOrder, TaskView, TuiDatabaseStats, TuiSyncStatus};
use crate::tui::task_selection::TaskSelection;
use crate::tui::text::{char_boundary_at_or_before, normalize_pasted_newlines};
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
    Detail {
        scroll: u16,
    },
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
        task_id: crate::ids::TaskId,
        display_ref: String,
    },
    AddEpicChild {
        epic_id: crate::ids::TaskId,
        display_ref: String,
        project_key: String,
        return_to_detail: bool,
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
pub(crate) struct CommandState {
    pub(crate) input: LineEdit,
    pub(crate) cycle_input: Option<String>,
    pub(crate) cycle_index: usize,
    pub(crate) highlighted: Option<String>,
}

impl CommandState {
    pub(crate) fn blank() -> Self {
        Self {
            input: LineEdit::blank(),
            cycle_input: None,
            cycle_index: 0,
            highlighted: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new(input: LineEdit) -> Self {
        Self {
            input,
            cycle_input: None,
            cycle_index: 0,
            highlighted: None,
        }
    }

    pub(crate) fn reset_cycle(&mut self) {
        self.cycle_input = None;
        self.cycle_index = 0;
        self.highlighted = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelState {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateOverlayState {
    Checking,
    Progress {
        version: String,
        phase: crate::update::UpdatePhase,
        cancelling: bool,
    },
    Current {
        version: String,
        cached: bool,
    },
    Guidance {
        version: String,
        lines: Vec<String>,
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
    AddLabel,
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
        return_to_detail: bool,
    },
    EditDue {
        selection: TaskSelection,
        mixed: bool,
        return_to_detail: bool,
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
        return_to_detail: bool,
    },
    EditDescription {
        selection: TaskSelection,
    },
    ResolveConflictManually {
        target: ConflictTarget,
    },
}

impl MultilineIntent {
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
    FilterLabel,
    FilterPriority,
    ScopeProject,
    RenameProject,
    DeleteProject,
    SwitchWorkspace,
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
        task_id: crate::ids::TaskId,
    },
}

impl PickerIntent {
    pub(crate) fn initial_mode(&self) -> PickerMode {
        match self {
            Self::AddTaskProject
            | Self::EditProject { .. }
            | Self::ScopeProject
            | Self::RenameProject
            | Self::DeleteProject => PickerMode::Filter,
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
    DeleteTasks {
        selection: TaskSelection,
        return_to_detail: bool,
    },
    DeleteAttachment {
        attachment_id: String,
        return_to_detail: bool,
        detail_scroll: u16,
    },
    ClearAvailability {
        selection: TaskSelection,
        return_to_detail: bool,
    },
    ClearDue {
        selection: TaskSelection,
        return_to_detail: bool,
    },
    InstallUpdate {
        plan: crate::update::InstallPlan,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddTaskMode {
    Compose,
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
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) available_at: LineEdit,
    pub(crate) due_on: LineEdit,
    pub(crate) attachments: Vec<PendingTaskAttachmentSummary>,
    pub(crate) selected_attachment: usize,
    pub(crate) mode: AddTaskMode,
    pub(crate) title_error: bool,
}

impl AddTaskState {
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
            || !self.available_at.text.trim().is_empty()
            || !self.due_on.text.trim().is_empty()
            || !self.attachments.is_empty()
    }

    pub(crate) fn focus_next(&mut self, reverse: bool) {
        self.focus = self.focus.next(reverse);
        if self.focus == AddTaskStep::Images && self.attachments.is_empty() {
            self.focus = self.focus.next(reverse);
        }
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
}

impl MultilineInputState {
    pub(crate) fn has_meaningful_content(&self) -> bool {
        self.lines.iter().any(|line| !line.trim().is_empty())
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
        Self {
            intent,
            title: title.into(),
            prompt: prompt.into(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
            mode: MultilineInputMode::Compose,
        }
    }

    pub(crate) fn from_value(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
    ) -> Self {
        let mut lines = value.split('\n').map(str::to_string).collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }
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
            return_to_detail: true,
        });
        assert!(matches!(
            search.intent,
            SearchIntent::AddEpicChild {
                ref display_ref,
                ref project_key,
                return_to_detail: true,
                ..
            } if display_ref == "APP-1234" && project_key == "app"
        ));

        let confirm = ConfirmState::new(
            ConfirmIntent::DeleteAttachment {
                attachment_id: "attachment-1".to_string(),
                return_to_detail: true,
                detail_scroll: 7,
            },
            "Delete attachment",
            "Delete attachment?",
        );
        assert!(matches!(
            confirm.intent,
            ConfirmIntent::DeleteAttachment {
                ref attachment_id,
                return_to_detail: true,
                detail_scroll: 7,
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
}

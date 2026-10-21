use crate::ids::{TaskId, WorkspaceId};
use crate::tui::conflict_flow::ConflictResolutionChoice;
use crate::tui::overlay::text_buffer::TextBuffer;
use crate::tui::overlay::text_input::LineEdit;
use crate::tui::store::{ConflictTarget, EpicChildTarget, EpicContext};
use crate::tui::task_selection::TaskSelection;
use crate::tui::text::normalize_pasted_newlines;
use aven_core::recurrence::RecurrenceSeriesId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayTarget {
    RecurrenceSeries {
        workspace_id: WorkspaceId,
        series_id: RecurrenceSeriesId,
    },
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
    ConfirmDeleteWorkspace {
        workspace: crate::workspaces::Workspace,
        warnings: Vec<String>,
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
    DeleteWorkspace,
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
    GoToBlocker {
        source_task_id: TaskId,
        scroll: u16,
    },
    RemoveRelated {
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
            | Self::RenameWorkspace
            | Self::DeleteWorkspace => PickerMode::Filter,
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
pub(crate) struct EpicChildRemovalRestoration {
    pub(crate) anchor_id: crate::ids::TaskId,
    pub(crate) section: crate::tui::app::DetailSection,
    pub(crate) scroll: u16,
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
    UnlinkRelated {
        selection: TaskSelection,
        related_task_id: crate::ids::TaskId,
    },
    UnlinkEpicChild {
        target: EpicChildTarget,
        restoration: EpicChildRemovalRestoration,
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
    pub(crate) buffer: TextBuffer,
    pub(crate) mode: MultilineInputMode,
}

impl MultilineInputState {
    pub(crate) fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
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
        self.buffer.baseline_value()
    }

    pub(crate) fn insert_paste(&mut self, text: &str) {
        self.buffer.insert_exact(&normalize_pasted_newlines(text));
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
        Self {
            intent,
            title: title.into(),
            prompt: prompt.into(),
            buffer: TextBuffer::from_value_with_baseline(value, baseline),
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

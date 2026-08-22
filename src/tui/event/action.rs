use crossterm::event::KeyCode;

use crate::choices::{TaskPriority, TaskStatus};
use crate::tui::store::{TaskOrder, TaskView};

#[cfg(test)]
use super::{ShortcutLookup, resolve_shortcut};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BulkSupport {
    Batch,
    SingleOnly(&'static str),
    Focused,
    BulkControl,
    NotTaskScoped,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
    MoveDown,
    MoveUp,
    MoveLeft,
    MoveRight,
    MoveColumnLeft,
    MoveColumnRight,
    BeginMoveToColumn,
    PreviousItem,
    NextItem,
    First,
    Last,
    ToggleFocus,
    ToggleSidebar,
    ToggleDetail,
    ToggleColumnsPreview,
    GoBack,
    GoForward,
    ReturnToLastChange,
    ToggleHelp,
    ShowWelcome,
    BeginSearch,
    BeginCommand,
    AcceptSearch,
    AcceptCommand,
    CancelOverlay,
    CancelSearch,
    CancelCommand,
    BackspaceSearch,
    BackspaceCommand,
    SearchChar(char),
    CommandChar(char),
    Refresh,
    SyncNow,
    SetOrder(TaskOrder),
    ReverseSort,
    SetStatus(TaskStatus),
    SetPriority(TaskPriority),
    CyclePriority(bool),
    CopyShortRef,
    CopyDurableRef,
    CopyTaskTitle,
    CopyTaskDescription,
    CopyTaskText,
    CopyTaskNotes,
    CopyTaskMarkdown,
    BeginCreateTaskGist,
    BeginEditTitle,
    BeginEditDescription,
    BeginEditProject,
    BeginEditPriority,
    BeginEditEpic,
    BeginEditAvailability,
    BeginEditDue,
    BeginEditLabels,
    SkipRecurrence,
    BeginEditRecurrenceTemplate,
    PauseRecurrence,
    ResumeRecurrence,
    StopRecurrence,
    ShowRecurrenceHistory,
    Delete,
    Restore,
    ToggleEpicExpanded,
    BeginAddEpicChild,
    RemoveEpicChild,
    BeginStatusPicker,
    BeginRenameProject,
    BeginDeleteProject,
    BeginAddTask,
    BeginAddNote,
    BeginAddProject,
    BeginAddProjectPath,
    BeginRemoveProjectPath,
    BeginAddLabel,
    BeginBrowseLabels,
    BeginRenameLabel,
    BeginDeleteLabel,
    BeginFilterLabel,
    BeginFilterPriority,
    BeginScopeProject,
    BeginSwitchWorkspace,
    BeginAddWorkspace,
    BeginRenameWorkspace,
    ClearFilters,
    ToggleClosedFilter,
    ToggleDeletedFilter,
    CycleRecurringLifecycleFilter,
    ShowView(TaskView),
    ShowWorkspaceScope,
    BeginConflictList,
    ShowConflictDetails,
    NextConflict,
    PreviousConflict,
    AcceptConflictLocal,
    AcceptConflictRemote,
    BeginManualConflictMerge,
    ShowConfigStatus,
    ShowConfigInfo,
    ShowConfigPaths,
    ShowDatabaseStats,
    BeginUpdate,
    ShowChangelog,
    BeginConfigInit,
    BeginAddDependency,
    BeginRemoveDependency,
    BeginAddRelated,
    BeginRemoveRelated,
    Undo,
    ToggleMarkSelected,
    ToggleMarkAllInView,
    ClearMarks,
    None,
}

pub(crate) const SINGLE_TASK_COPY_ACTIONS: [Action; 4] = [
    Action::CopyTaskDescription,
    Action::CopyTaskText,
    Action::CopyTaskNotes,
    Action::CopyTaskMarkdown,
];

impl Action {
    pub(crate) const fn copy_requires_single_task(self) -> bool {
        matches!(
            self,
            Self::CopyTaskDescription
                | Self::CopyTaskText
                | Self::CopyTaskNotes
                | Self::CopyTaskMarkdown
        )
    }

    pub(crate) const fn bulk_support(self) -> BulkSupport {
        match self {
            Self::MoveColumnLeft
            | Self::MoveColumnRight
            | Self::BeginMoveToColumn
            | Self::SetStatus(_)
            | Self::SetPriority(_)
            | Self::CyclePriority(_)
            | Self::BeginEditProject
            | Self::BeginEditPriority
            | Self::BeginEditEpic
            | Self::BeginEditAvailability
            | Self::BeginEditDue
            | Self::BeginEditLabels
            | Self::Delete
            | Self::Restore
            | Self::BeginStatusPicker => BulkSupport::Batch,
            Self::BeginEditTitle => BulkSupport::SingleOnly("title"),
            Self::BeginEditDescription => BulkSupport::SingleOnly("description"),
            Self::BeginAddNote => BulkSupport::SingleOnly("note"),
            Self::BeginAddDependency | Self::BeginRemoveDependency => {
                BulkSupport::SingleOnly("dependency")
            }
            Self::BeginAddRelated | Self::BeginRemoveRelated => {
                BulkSupport::SingleOnly("related link")
            }
            Self::CopyShortRef | Self::CopyDurableRef | Self::CopyTaskTitle => BulkSupport::Batch,
            Self::CopyTaskDescription
            | Self::CopyTaskText
            | Self::CopyTaskNotes
            | Self::CopyTaskMarkdown => BulkSupport::SingleOnly("copy"),
            Self::BeginCreateTaskGist
            | Self::SkipRecurrence
            | Self::BeginEditRecurrenceTemplate
            | Self::PauseRecurrence
            | Self::ResumeRecurrence
            | Self::StopRecurrence
            | Self::ShowRecurrenceHistory
            | Self::ToggleEpicExpanded
            | Self::BeginAddEpicChild
            | Self::RemoveEpicChild => BulkSupport::Focused,
            Self::Undo
            | Self::ToggleMarkSelected
            | Self::ToggleMarkAllInView
            | Self::ClearMarks => BulkSupport::BulkControl,
            Self::Quit
            | Self::MoveDown
            | Self::MoveUp
            | Self::MoveLeft
            | Self::MoveRight
            | Self::PreviousItem
            | Self::NextItem
            | Self::First
            | Self::Last
            | Self::ToggleFocus
            | Self::ToggleSidebar
            | Self::ToggleDetail
            | Self::ToggleColumnsPreview
            | Self::GoBack
            | Self::GoForward
            | Self::ReturnToLastChange
            | Self::ToggleHelp
            | Self::ShowWelcome
            | Self::BeginSearch
            | Self::BeginCommand
            | Self::AcceptSearch
            | Self::AcceptCommand
            | Self::CancelOverlay
            | Self::CancelSearch
            | Self::CancelCommand
            | Self::BackspaceSearch
            | Self::BackspaceCommand
            | Self::SearchChar(_)
            | Self::CommandChar(_)
            | Self::Refresh
            | Self::SyncNow
            | Self::SetOrder(_)
            | Self::ReverseSort
            | Self::BeginRenameProject
            | Self::BeginDeleteProject
            | Self::BeginAddTask
            | Self::BeginAddProject
            | Self::BeginAddProjectPath
            | Self::BeginRemoveProjectPath
            | Self::BeginAddLabel
            | Self::BeginBrowseLabels
            | Self::BeginRenameLabel
            | Self::BeginDeleteLabel
            | Self::BeginFilterLabel
            | Self::BeginFilterPriority
            | Self::BeginScopeProject
            | Self::BeginSwitchWorkspace
            | Self::BeginAddWorkspace
            | Self::BeginRenameWorkspace
            | Self::ClearFilters
            | Self::ToggleClosedFilter
            | Self::ToggleDeletedFilter
            | Self::CycleRecurringLifecycleFilter
            | Self::ShowView(_)
            | Self::ShowWorkspaceScope
            | Self::BeginConflictList
            | Self::ShowConflictDetails
            | Self::NextConflict
            | Self::PreviousConflict
            | Self::AcceptConflictLocal
            | Self::AcceptConflictRemote
            | Self::BeginManualConflictMerge
            | Self::ShowConfigStatus
            | Self::ShowConfigInfo
            | Self::ShowConfigPaths
            | Self::ShowDatabaseStats
            | Self::BeginUpdate
            | Self::ShowChangelog
            | Self::BeginConfigInit
            | Self::None => BulkSupport::NotTaskScoped,
        }
    }

    pub(crate) const fn recurrence_kind(
        self,
    ) -> Option<crate::tui::app_recurrence::RecurrenceActionKind> {
        use crate::tui::app_recurrence::RecurrenceActionKind;

        match self {
            Self::SkipRecurrence => Some(RecurrenceActionKind::SkipCurrent),
            Self::BeginEditRecurrenceTemplate => Some(RecurrenceActionKind::EditTemplate),
            Self::PauseRecurrence => Some(RecurrenceActionKind::Pause),
            Self::ResumeRecurrence => Some(RecurrenceActionKind::Resume),
            Self::StopRecurrence => Some(RecurrenceActionKind::Stop),
            Self::ShowRecurrenceHistory => Some(RecurrenceActionKind::History),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn from_search_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Esc => Self::CancelSearch,
            KeyCode::Enter => Self::AcceptSearch,
            KeyCode::Backspace => Self::BackspaceSearch,
            KeyCode::Char(ch) => Self::SearchChar(ch),
            _ => Self::None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn from_command_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Esc => Self::CancelCommand,
            KeyCode::Enter => Self::AcceptCommand,
            KeyCode::Backspace => Self::BackspaceCommand,
            KeyCode::Char(ch) => Self::CommandChar(ch),
            _ => Self::None,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_normal_key(code: KeyCode) -> Self {
        if code == KeyCode::Esc {
            return Self::CancelOverlay;
        }

        match resolve_shortcut(&[code]) {
            ShortcutLookup::Found(action) | ShortcutLookup::Ambiguous(action) => action,
            ShortcutLookup::Prefix | ShortcutLookup::Missing => Self::None,
        }
    }
}

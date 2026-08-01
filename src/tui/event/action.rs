use crossterm::event::KeyCode;

use crate::choices::{TaskPriority, TaskStatus};
use crate::tui::store::{TaskOrder, TaskView};

#[cfg(test)]
use super::{ShortcutLookup, resolve_shortcut};

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
    Undo,
    ToggleMarkSelected,
    ToggleMarkAllInView,
    ClearMarks,
    Planned {
        name: &'static str,
        reason: &'static str,
    },
    Disabled {
        name: &'static str,
        reason: &'static str,
    },
    None,
}

impl Action {
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

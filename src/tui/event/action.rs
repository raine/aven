use crossterm::event::KeyCode;

use crate::choices::{TaskPriority, TaskStatus};
use crate::tui::store::{TaskLayout, TaskOrder, TaskQuery};

#[cfg(test)]
use super::{ShortcutLookup, resolve_shortcut_for};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BulkSupport {
    Batch,
    SingleOnly(&'static str),
    Focused,
    BulkControl,
    NotTaskScoped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceEffect {
    Preserve,
    ExitDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationshipTargetPolicy {
    Dependency,
    Related,
    EpicChild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandTargetPolicy {
    None,
    Focused,
    Single(&'static str),
    Batch,
    Marks,
    Relationship(RelationshipTargetPolicy),
    Attachment,
    Recurrence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandScopePolicy {
    Any,
    ListOnly,
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
    OpenAttachment,
    SaveAttachment,
    DeleteAttachment,
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
    ToggleLayout,
    SetLayout(TaskLayout),
    ShowView(TaskQuery),
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

    pub(crate) const fn surface_effect(self) -> SurfaceEffect {
        if matches!(
            self,
            Self::SetOrder(_)
                | Self::ReverseSort
                | Self::BeginScopeProject
                | Self::BeginSwitchWorkspace
                | Self::ClearFilters
                | Self::ToggleClosedFilter
                | Self::ToggleDeletedFilter
                | Self::CycleRecurringLifecycleFilter
                | Self::ToggleLayout
                | Self::SetLayout(_)
                | Self::ShowView(_)
                | Self::ShowWorkspaceScope
                | Self::BeginConflictList
                | Self::NextConflict
                | Self::PreviousConflict
        ) {
            SurfaceEffect::ExitDetail
        } else {
            SurfaceEffect::Preserve
        }
    }

    pub(crate) const fn scope_policy(self) -> CommandScopePolicy {
        if matches!(
            self,
            Self::MoveDown
                | Self::MoveUp
                | Self::MoveLeft
                | Self::MoveRight
                | Self::PreviousItem
                | Self::NextItem
                | Self::First
                | Self::Last
                | Self::ToggleFocus
                | Self::ToggleSidebar
                | Self::ToggleColumnsPreview
                | Self::BeginFilterLabel
                | Self::BeginFilterPriority
                | Self::ToggleMarkSelected
                | Self::ToggleMarkAllInView
                | Self::ClearMarks
        ) {
            CommandScopePolicy::ListOnly
        } else {
            CommandScopePolicy::Any
        }
    }

    pub(crate) const fn target_policy(self) -> CommandTargetPolicy {
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
            | Self::BeginStatusPicker
            | Self::CopyShortRef
            | Self::CopyDurableRef
            | Self::CopyTaskTitle => CommandTargetPolicy::Batch,
            Self::BeginEditTitle => CommandTargetPolicy::Single("title"),
            Self::BeginEditDescription => CommandTargetPolicy::Single("description"),
            Self::BeginAddNote => CommandTargetPolicy::Single("note"),
            Self::BeginAddDependency => CommandTargetPolicy::Single("dependency"),
            Self::BeginAddRelated => CommandTargetPolicy::Single("related link"),
            Self::CopyTaskDescription
            | Self::CopyTaskText
            | Self::CopyTaskNotes
            | Self::CopyTaskMarkdown => CommandTargetPolicy::Single("copy"),
            Self::BeginRemoveDependency => {
                CommandTargetPolicy::Relationship(RelationshipTargetPolicy::Dependency)
            }
            Self::BeginRemoveRelated => {
                CommandTargetPolicy::Relationship(RelationshipTargetPolicy::Related)
            }
            Self::RemoveEpicChild => {
                CommandTargetPolicy::Relationship(RelationshipTargetPolicy::EpicChild)
            }
            Self::OpenAttachment | Self::SaveAttachment | Self::DeleteAttachment => {
                CommandTargetPolicy::Attachment
            }
            Self::SkipRecurrence
            | Self::BeginEditRecurrenceTemplate
            | Self::PauseRecurrence
            | Self::ResumeRecurrence
            | Self::StopRecurrence
            | Self::ShowRecurrenceHistory => CommandTargetPolicy::Recurrence,
            Self::BeginCreateTaskGist
            | Self::ToggleEpicExpanded
            | Self::BeginAddEpicChild
            | Self::ShowConflictDetails
            | Self::AcceptConflictLocal
            | Self::AcceptConflictRemote
            | Self::BeginManualConflictMerge
            | Self::ToggleDetail => CommandTargetPolicy::Focused,
            Self::ToggleMarkSelected | Self::ToggleMarkAllInView | Self::ClearMarks => {
                CommandTargetPolicy::Marks
            }
            _ => CommandTargetPolicy::None,
        }
    }

    pub(crate) const fn bulk_support(self) -> BulkSupport {
        match self.target_policy() {
            CommandTargetPolicy::Batch => BulkSupport::Batch,
            CommandTargetPolicy::Single(reason) => BulkSupport::SingleOnly(reason),
            CommandTargetPolicy::Focused
            | CommandTargetPolicy::Relationship(_)
            | CommandTargetPolicy::Attachment
            | CommandTargetPolicy::Recurrence => BulkSupport::Focused,
            CommandTargetPolicy::Marks => BulkSupport::BulkControl,
            CommandTargetPolicy::None => BulkSupport::NotTaskScoped,
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

        match resolve_shortcut_for(super::CommandContext::Normal, &[code]) {
            ShortcutLookup::Found(action) | ShortcutLookup::Ambiguous(action) => action,
            ShortcutLookup::Prefix | ShortcutLookup::Missing => Self::None,
        }
    }
}

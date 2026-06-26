use crossterm::event::KeyCode;

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
    PreviousItem,
    NextItem,
    First,
    Last,
    ToggleFocus,
    ToggleDetail,
    ToggleHelp,
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
    SetOrder(TaskOrder),
    ReverseSort,
    SetStatus(&'static str),
    SetPriority(&'static str),
    CyclePriority(bool),
    CopyShortRef,
    CopyDurableRef,
    BeginEditTitle,
    BeginEditDescription,
    BeginEditProject,
    BeginEditPriority,
    BeginEditLabels,
    Delete,
    Restore,
    BeginStatusPicker,
    BeginDeleteProject,
    BeginAddTask,
    BeginAddNote,
    BeginAddProject,
    BeginAddLabel,
    BeginFilterLabel,
    BeginFilterPriority,
    BeginScopeProject,
    BeginSwitchWorkspace,
    ClearFilters,
    ToggleDeletedFilter,
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
    BeginConfigInit,
    Undo,
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

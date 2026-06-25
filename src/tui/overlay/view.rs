use crate::tui::authoring::AddTaskStep;
use crate::tui::store::TuiSyncStatus;

use super::picker::visible_picker_indices;
use super::state::{OverlayRoute, OverlayState, OverlayState::*, PickerItem, PickerMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayView {
    Help {
        scroll: u16,
    },
    Detail {
        scroll: u16,
    },
    DetailHelp {
        scroll: u16,
    },
    Search {
        input: String,
        cursor: usize,
    },
    Command {
        input: String,
        cursor: usize,
        cycle_input: Option<String>,
        highlighted: Option<String>,
    },
    AddTask(AddTaskView),
    TextInput(TextInputView),
    MultilineInput(MultilineInputView),
    Picker(PickerView),
    Confirm(ConfirmView),
    TextPanel(TextPanelView),
    SyncStatus(Box<TuiSyncStatus>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelView {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskView {
    pub(crate) title: String,
    pub(crate) title_cursor: usize,
    pub(crate) description: Vec<String>,
    pub(crate) description_row: usize,
    pub(crate) description_column: usize,
    pub(crate) focus: AddTaskStep,
    pub(crate) project: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) status_prefix_active: bool,
    pub(crate) priority_prefix_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextInputView {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultilineInputView {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerView {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) filter: String,
    pub(crate) filter_cursor: usize,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
    pub(crate) multi: bool,
    pub(crate) mode: PickerMode,
    pub(crate) visible_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmView {
    pub(crate) title: String,
    pub(crate) prompt: String,
}

impl From<&OverlayState> for OverlayView {
    fn from(state: &OverlayState) -> Self {
        match state {
            Help { scroll } => Self::Help { scroll: *scroll },
            Detail { scroll } => Self::Detail { scroll: *scroll },
            DetailHelp { scroll } => Self::DetailHelp { scroll: *scroll },
            Search { input } => Self::Search {
                input: input.text.clone(),
                cursor: input.cursor,
            },
            Command { state } => Self::Command {
                input: state.input.text.clone(),
                cursor: state.input.cursor,
                cycle_input: state.cycle_input.clone(),
                highlighted: state.highlighted.clone(),
            },
            AddTask(state) => Self::AddTask(AddTaskView {
                title: state.title.text.clone(),
                title_cursor: state.title.cursor,
                description: state.description.lines.clone(),
                description_row: state.description.row,
                description_column: state.description.column,
                focus: state.focus,
                project: state.project.clone(),
                status: state.status.clone(),
                priority: state.priority.clone(),
                status_prefix_active: false,
                priority_prefix_active: false,
            }),
            TextInput(state) => Self::TextInput(TextInputView {
                route: state.route,
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                input: state.input.text.clone(),
                cursor: state.input.cursor,
            }),
            MultilineInput(state) => Self::MultilineInput(MultilineInputView {
                route: state.route,
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                lines: state.lines.clone(),
                row: state.row,
                column: state.column,
            }),
            Picker(state) => Self::Picker(PickerView {
                route: state.route,
                title: state.title.clone(),
                filter: state.filter.text.clone(),
                filter_cursor: state.filter.cursor,
                items: state.items.clone(),
                selected: state.selected,
                multi: state.multi,
                mode: state.mode,
                visible_indices: visible_picker_indices(state),
            }),
            Confirm(state) => Self::Confirm(ConfirmView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
            }),
            TextPanel(state) => Self::TextPanel(TextPanelView {
                title: state.title.clone(),
                lines: state.lines.clone(),
                scroll: state.scroll,
            }),
            SyncStatus(state) => Self::SyncStatus(state.clone()),
        }
    }
}

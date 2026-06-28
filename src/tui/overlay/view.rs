use crate::tui::authoring::AddTaskStep;
use crate::tui::store::{TaskOrder, TuiDatabaseStats, TuiSyncStatus};

use super::layout::TAG_COMBOBOX_VIEWPORT_ROWS;
use super::picker::visible_picker_indices;
use super::state::{
    HeaderMenuItem, HeaderMenuKind, HeaderMenuState, OrderMenuState, OverlayRoute, OverlayState,
    OverlayState::*, PickerItem, PickerMode, SearchResultItem,
};
use super::tag_combobox::{tag_combobox_completion, tag_combobox_matches};

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
        results: Vec<SearchResultItem>,
        selected: usize,
        total_matches: usize,
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
    TagCombobox(TagComboboxView),
    HeaderMenu(HeaderMenuView),
    OrderMenu(OrderMenuView),
    Confirm(ConfirmView),
    TextPanel(TextPanelView),
    SyncStatus(Box<TuiSyncStatus>),
    DatabaseStats {
        stats: Box<TuiDatabaseStats>,
        scroll: u16,
    },
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
    pub(crate) scroll: usize,
    pub(crate) multi: bool,
    pub(crate) mode: PickerMode,
    pub(crate) visible_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagComboboxView {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) completion: Option<String>,
    pub(crate) options: Vec<String>,
    pub(crate) selected: Vec<String>,
    pub(crate) highlighted: usize,
    pub(crate) visible_indices: Vec<usize>,
    pub(crate) visible_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderMenuView {
    pub(crate) kind: HeaderMenuKind,
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) selected: usize,
    pub(crate) items: Vec<HeaderMenuItem>,
}

impl From<&HeaderMenuState> for HeaderMenuView {
    fn from(state: &HeaderMenuState) -> Self {
        Self {
            kind: state.kind,
            column: state.column,
            row: state.row,
            selected: state.selected,
            items: state.items.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrderMenuView {
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) selected: TaskOrder,
}

impl From<&OrderMenuState> for OrderMenuView {
    fn from(state: &OrderMenuState) -> Self {
        Self {
            column: state.column,
            row: state.row,
            selected: state.selected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmView {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) prompt: String,
}

impl From<&OverlayState> for OverlayView {
    fn from(state: &OverlayState) -> Self {
        match state {
            Help { scroll } => Self::Help { scroll: *scroll },
            Detail { scroll } => Self::Detail { scroll: *scroll },
            DetailHelp { scroll } => Self::DetailHelp { scroll: *scroll },
            Search(state) => Self::Search {
                input: state.input.text.clone(),
                cursor: state.input.cursor,
                results: state.results.clone(),
                selected: state.selected,
                total_matches: state.total_matches,
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
                scroll: state.scroll,
                multi: state.multi,
                mode: state.mode,
                visible_indices: visible_picker_indices(state),
            }),
            TagCombobox(state) => {
                let visible_indices = tag_combobox_matches(state);
                Self::TagCombobox(TagComboboxView {
                    route: state.route,
                    title: state.title.clone(),
                    input: state.input.text.clone(),
                    input_cursor: state.input.cursor,
                    completion: tag_combobox_completion(state),
                    options: state.options.clone(),
                    selected: state.selected.clone(),
                    highlighted: state.highlighted,
                    visible_start: visible_indices
                        .iter()
                        .position(|index| *index == state.highlighted)
                        .unwrap_or(0)
                        .saturating_sub(TAG_COMBOBOX_VIEWPORT_ROWS.saturating_sub(1)),
                    visible_indices,
                })
            }
            HeaderMenu(state) => Self::HeaderMenu(HeaderMenuView::from(state)),
            OrderMenu(state) => Self::OrderMenu(OrderMenuView::from(state)),
            Confirm(state) => Self::Confirm(ConfirmView {
                route: state.route,
                title: state.title.clone(),
                prompt: state.prompt.clone(),
            }),
            TextPanel(state) => Self::TextPanel(TextPanelView {
                title: state.title.clone(),
                lines: state.lines.clone(),
                scroll: state.scroll,
            }),
            SyncStatus(state) => Self::SyncStatus(state.clone()),
            DatabaseStats { stats, scroll } => Self::DatabaseStats {
                stats: stats.clone(),
                scroll: *scroll,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::{
        ConfirmState, LineEdit, MultilineInputState, OverlayRoute, PickerState,
    };

    #[test]
    fn overlay_view_projection_carries_routes() {
        let multiline = OverlayView::from(&OverlayState::MultilineInput(
            MultilineInputState::blank(OverlayRoute::AddNote, "Changed note title", "note body:"),
        ));
        assert!(matches!(
            multiline,
            OverlayView::MultilineInput(MultilineInputView {
                route: OverlayRoute::AddNote,
                ..
            })
        ));

        let picker = OverlayView::from(&OverlayState::Picker(PickerState {
            route: OverlayRoute::DeleteProjectPicker,
            title: "Changed delete title".to_string(),
            filter: LineEdit::blank(),
            items: vec![PickerItem {
                label: "AVN aven".to_string(),
                value: "aven".to_string(),
                selected: false,
            }],
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        }));
        assert!(matches!(
            picker,
            OverlayView::Picker(PickerView {
                route: OverlayRoute::DeleteProjectPicker,
                ..
            })
        ));

        let confirm = OverlayView::from(&OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::DeleteTaskConfirm,
            title: "Delete task".to_string(),
            prompt: "Delete task?".to_string(),
        }));
        assert!(matches!(
            confirm,
            OverlayView::Confirm(ConfirmView {
                route: OverlayRoute::DeleteTaskConfirm,
                ..
            })
        ));
    }
}

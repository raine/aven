use crate::tui::authoring::{AddTaskStep, PendingTaskAttachmentSummary};
use crate::tui::store::{TaskOrder, TuiDatabaseStats, TuiSyncStatus};

use super::layout::TAG_COMBOBOX_VIEWPORT_ROWS;
use super::picker::visible_picker_indices;
use super::state::{
    AddTaskMode, HeaderMenuItem, HeaderMenuKind, HeaderMenuState, MultilineInputMode,
    MultilineIntent, OrderMenuState, OverlayState, OverlayState::*, PickerIntent, PickerItem,
    PickerMode, SearchIntent, SearchResultItem, TagComboboxIntent, TextIntent,
};
use super::tag_combobox::{tag_combobox_completion, tag_combobox_matches};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchKind {
    Navigate,
    AddDependency,
    AddEpicChild { display_ref: String },
}

impl From<&SearchIntent> for SearchKind {
    fn from(intent: &SearchIntent) -> Self {
        match intent {
            SearchIntent::Navigate => Self::Navigate,
            SearchIntent::AddDependency { .. } => Self::AddDependency,
            SearchIntent::AddEpicChild { display_ref, .. } => Self::AddEpicChild {
                display_ref: display_ref.clone(),
            },
        }
    }
}

impl SearchKind {
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Navigate => "Search".to_string(),
            Self::AddDependency => "Add dependency".to_string(),
            Self::AddEpicChild { display_ref } => format!("Add child to {display_ref}"),
        }
    }

    pub(crate) fn enter_hint(&self) -> &'static str {
        match self {
            Self::Navigate => "open task",
            Self::AddDependency => "add selected as blocker",
            Self::AddEpicChild { .. } => "add selected child",
        }
    }

    pub(crate) fn placeholder(&self) -> &'static str {
        match self {
            Self::Navigate => "Search tasks, notes, labels, and projects...",
            Self::AddDependency => "Search for the task that blocks this task...",
            Self::AddEpicChild { .. } => "Search for an existing task or create a child...",
        }
    }

    pub(crate) fn tab_hint(&self) -> Option<&'static str> {
        match self {
            Self::Navigate => Some("open results"),
            Self::AddDependency | Self::AddEpicChild { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayView {
    Onboarding {
        splash_underlay: bool,
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
    Search {
        input: String,
        cursor: usize,
        results: Vec<SearchResultItem>,
        selected: usize,
        total_matches: usize,
        stale: bool,
        no_matches_cached: bool,
        intent: SearchKind,
    },
    Command {
        input: String,
        cursor: usize,
        cycle_input: Option<String>,
        highlighted: Option<String>,
        context: crate::tui::event::CommandContext,
        unavailable: Vec<super::state::CommandAvailabilityOverride>,
    },
    AddTask(Box<AddTaskView>),
    TextInput(TextInputView),
    MultilineInput(MultilineInputView),
    Picker(PickerView),
    TagCombobox(Box<TagComboboxView>),
    HeaderMenu(HeaderMenuView),
    OrderMenu(OrderMenuView),
    Confirm(ConfirmView),
    TextPanel(TextPanelView),
    Changelog {
        markdown: String,
        scroll: u16,
    },
    RecurrenceHistory(RecurrenceHistoryView),
    SyncStatus(Box<TuiSyncStatus>),
    DatabaseStats {
        stats: Box<TuiDatabaseStats>,
        scroll: u16,
    },
    Update(super::state::UpdateOverlayState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelView {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskAttachmentsView {
    pub(crate) items: Box<[PendingTaskAttachmentSummary]>,
    pub(crate) selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecurrenceHistoryView {
    pub(crate) page: aven_core::query::RecurrenceHistoryPage,
    pub(crate) selected: Option<usize>,
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
    pub(crate) labels: Vec<String>,
    pub(crate) available_at: String,
    pub(crate) available_at_cursor: usize,
    pub(crate) due_on: String,
    pub(crate) due_on_cursor: usize,
    pub(crate) schedule_input: String,
    pub(crate) schedule_input_cursor: usize,
    pub(crate) schedule_error: Option<String>,
    pub(crate) schedule_validation_requested: bool,
    pub(crate) attachments: Box<AddTaskAttachmentsView>,
    pub(crate) recurrence_series_id: Option<aven_core::recurrence::RecurrenceSeriesId>,
    pub(crate) editing_template: bool,
    pub(crate) repeat_rule: String,
    pub(crate) repeat_rule_cursor: usize,
    pub(crate) repeat_at: String,
    pub(crate) repeat_at_cursor: usize,
    pub(crate) repeat_due: String,
    pub(crate) time_zone: String,
    pub(crate) repeat_start_on: String,
    pub(crate) repeat_start_on_cursor: usize,
    pub(crate) schedule_expanded: bool,
    pub(crate) recurrence_preview: Vec<String>,
    pub(crate) recurrence_error: Option<String>,
    pub(crate) mode: Box<AddTaskMode>,
    pub(crate) title_error: bool,
    pub(crate) status_prefix_active: bool,
    pub(crate) priority_prefix_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextInputKind {
    AddProject,
    ProjectPath,
    AddLabel,
    RenameProject,
    ConfirmDeleteProject,
    EditTitle,
    EditDate,
    ConflictManual,
}

impl From<&TextIntent> for TextInputKind {
    fn from(intent: &TextIntent) -> Self {
        match intent {
            TextIntent::AddProject => Self::AddProject,
            TextIntent::AddProjectPath { .. } => Self::ProjectPath,
            TextIntent::AddLabel => Self::AddLabel,
            TextIntent::RenameProject { .. } => Self::RenameProject,
            TextIntent::ConfirmDeleteProject { .. } => Self::ConfirmDeleteProject,
            TextIntent::EditTitle { .. } => Self::EditTitle,
            TextIntent::EditAvailability { .. } | TextIntent::EditDue { .. } => Self::EditDate,
            TextIntent::ResolveConflictManually { .. } => Self::ConflictManual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextInputView {
    pub(crate) kind: TextInputKind,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MultilineInputKind {
    AddTaskDescription,
    AddTaskNatural,
    AddNote,
    EditNote,
    EditDescription,
    ConflictManual,
}

impl From<&MultilineIntent> for MultilineInputKind {
    fn from(intent: &MultilineIntent) -> Self {
        match intent {
            MultilineIntent::AddTaskDescription => Self::AddTaskDescription,
            MultilineIntent::AddTaskNatural => Self::AddTaskNatural,
            MultilineIntent::AddNote { .. } => Self::AddNote,
            MultilineIntent::EditNote { .. } => Self::EditNote,
            MultilineIntent::EditDescription { .. } => Self::EditDescription,
            MultilineIntent::ResolveConflictManually { .. } => Self::ConflictManual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultilineInputView {
    pub(crate) kind: MultilineInputKind,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) mode: MultilineInputMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerKind {
    AddTaskProject,
    AddTaskPriority,
    EditProject,
    ScopeProject,
    ProjectPathProject,
    DeleteProject,
    EditPriority,
    Generic,
}

impl From<&PickerIntent> for PickerKind {
    fn from(intent: &PickerIntent) -> Self {
        match intent {
            PickerIntent::AddTaskProject => Self::AddTaskProject,
            PickerIntent::AddTaskPriority => Self::AddTaskPriority,
            PickerIntent::EditProject { .. } => Self::EditProject,
            PickerIntent::ScopeProject => Self::ScopeProject,
            PickerIntent::AddProjectPath | PickerIntent::RemoveProjectPath => {
                Self::ProjectPathProject
            }
            PickerIntent::DeleteProject => Self::DeleteProject,
            PickerIntent::EditPriority { .. } => Self::EditPriority,
            _ => Self::Generic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerView {
    pub(crate) kind: PickerKind,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagComboboxKind {
    AddTaskLabels,
    EditLabels,
    EditLabelsMulti,
}

impl From<&TagComboboxIntent> for TagComboboxKind {
    fn from(intent: &TagComboboxIntent) -> Self {
        match intent {
            TagComboboxIntent::AddTaskLabels => Self::AddTaskLabels,
            TagComboboxIntent::EditLabels { .. } => Self::EditLabels,
            TagComboboxIntent::EditLabelsMulti { .. } => Self::EditLabelsMulti,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagComboboxView {
    pub(crate) kind: TagComboboxKind,
    pub(crate) title: String,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) completion: Option<String>,
    pub(crate) options: Vec<String>,
    pub(crate) selected: Vec<String>,
    pub(crate) partial: Vec<String>,
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
    pub(crate) title: String,
    pub(crate) prompt: String,
}

impl From<&OverlayState> for OverlayView {
    fn from(state: &OverlayState) -> Self {
        match state {
            Onboarding { persist_on_exit } => Self::Onboarding {
                splash_underlay: *persist_on_exit,
            },
            Help { scroll } => Self::Help { scroll: *scroll },
            Detail => Self::Detail { scroll: 0 },
            AttachmentPreview {
                attachment_id,
                scroll,
            } => Self::AttachmentPreview {
                attachment_id: attachment_id.clone(),
                scroll: *scroll,
            },
            DetailHelp { scroll } => Self::DetailHelp { scroll: *scroll },
            Search(state) => Self::Search {
                input: state.input.text.clone(),
                cursor: state.input.cursor,
                results: state.results.clone(),
                selected: state.selected,
                total_matches: state.total_matches,
                stale: !state.results_are_current(),
                no_matches_cached: state.results_query.is_some()
                    && state.results.is_empty()
                    && state.total_matches == 0,
                intent: (&state.intent).into(),
            },
            Command { state } => Self::Command {
                input: state.input.text.clone(),
                cursor: state.input.cursor,
                cycle_input: state.cycle_input.clone(),
                highlighted: state.highlighted.clone(),
                context: state.context,
                unavailable: state.unavailable.clone(),
            },
            AddTask(state) => Self::AddTask(Box::new(AddTaskView {
                title: state.title.text.clone(),
                title_cursor: state.title.cursor,
                description: state.description.lines.clone(),
                description_row: state.description.row,
                description_column: state.description.column,
                focus: state.focus,
                project: state.project.clone(),
                status: state.status.clone(),
                priority: state.priority.clone(),
                labels: state.labels.clone(),
                available_at: state.available_at.text.clone(),
                available_at_cursor: state.available_at.cursor,
                due_on: state.due_on.text.clone(),
                due_on_cursor: state.due_on.cursor,
                schedule_input: state.schedule_input.text.clone(),
                schedule_input_cursor: state.schedule_input.cursor,
                schedule_error: state.schedule_error.clone(),
                schedule_validation_requested: state.schedule_validation_requested,
                attachments: Box::new(AddTaskAttachmentsView {
                    items: state.attachments.clone().into_boxed_slice(),
                    selected: state.selected_attachment,
                }),
                recurrence_series_id: state.recurrence_series_id.clone(),
                editing_template: state.template_schedule.is_some(),
                repeat_rule: state.repeat_rule.text.clone(),
                repeat_rule_cursor: state.repeat_rule.cursor,
                repeat_at: state.repeat_at.text.clone(),
                repeat_at_cursor: state.repeat_at.cursor,
                repeat_due: state.repeat_due.clone(),
                time_zone: state.time_zone.clone(),
                repeat_start_on: state.repeat_start_on.text.clone(),
                repeat_start_on_cursor: state.repeat_start_on.cursor,
                schedule_expanded: state.schedule_expanded,
                recurrence_preview: state.recurrence_preview.clone(),
                recurrence_error: state.recurrence_error.clone(),
                mode: Box::new(state.mode.clone()),
                title_error: state.title_error,
                status_prefix_active: false,
                priority_prefix_active: false,
            })),
            TextInput(state) => Self::TextInput(TextInputView {
                kind: (&state.intent).into(),
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                input: state.input.text.clone(),
                cursor: state.input.cursor,
            }),
            MultilineInput(state) => Self::MultilineInput(MultilineInputView {
                kind: (&state.intent).into(),
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                lines: state.lines.clone(),
                row: state.row,
                column: state.column,
                mode: state.mode,
            }),
            Picker(state) => Self::Picker(PickerView {
                kind: (&state.intent).into(),
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
                Self::TagCombobox(Box::new(TagComboboxView {
                    kind: (&state.intent).into(),
                    title: state.title.clone(),
                    input: state.input.text.clone(),
                    input_cursor: state.input.cursor,
                    completion: tag_combobox_completion(state),
                    options: state.options.clone(),
                    selected: state.selected.clone(),
                    partial: state.partial.clone(),
                    highlighted: state.highlighted,
                    visible_start: visible_indices
                        .iter()
                        .position(|index| *index == state.highlighted)
                        .unwrap_or(0)
                        .saturating_sub(TAG_COMBOBOX_VIEWPORT_ROWS.saturating_sub(1)),
                    visible_indices,
                }))
            }
            HeaderMenu(state) => Self::HeaderMenu(HeaderMenuView::from(state)),
            OrderMenu(state) => Self::OrderMenu(OrderMenuView::from(state)),
            Confirm(state) => Self::Confirm(ConfirmView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
            }),
            TextPanel(state) => Self::TextPanel(TextPanelView {
                title: state.title.clone(),
                lines: state.lines.clone(),
                scroll: state.scroll,
            }),
            Changelog(state) => Self::Changelog {
                markdown: state.markdown.clone(),
                scroll: state.scroll,
            },
            RecurrenceHistory(state) => Self::RecurrenceHistory(RecurrenceHistoryView {
                page: state.page.clone(),
                selected: state.selected,
            }),
            SyncStatus(state) => Self::SyncStatus(state.clone()),
            DatabaseStats { stats, scroll } => Self::DatabaseStats {
                stats: stats.clone(),
                scroll: *scroll,
            },
            Update(state) => Self::Update(state.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::{LineEdit, PickerIntent, PickerState};

    #[test]
    fn overlay_view_projection_keeps_picker_presentation_kind() {
        let picker = OverlayView::from(&OverlayState::Picker(PickerState {
            intent: PickerIntent::DeleteProject,
            title: "Delete project".to_string(),
            filter: LineEdit::blank(),
            items: vec![PickerItem {
                label: "AVN aven".to_string(),
                value: "aven".to_string(),
                selected: false,
            }],
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Filter,
        }));
        assert!(matches!(
            picker,
            OverlayView::Picker(PickerView {
                kind: PickerKind::DeleteProject,
                ..
            })
        ));
    }
}

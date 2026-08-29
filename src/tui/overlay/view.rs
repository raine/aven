use crate::tui::authoring::{AddTaskStep, PendingTaskAttachmentSummary};
use crate::tui::store::{TaskOrder, TuiDatabaseStats, TuiSyncStatus};

use super::layout::TAG_COMBOBOX_VIEWPORT_ROWS;
use super::picker::visible_picker_indices;
use super::state::{
    AddTaskMode, HeaderMenuItem, HeaderMenuKind, HeaderMenuState, MultilineInputMode,
    MultilineIntent, OrderMenuState, OverlayState, OverlayState::*, PickerIntent, PickerItem,
    PickerMode, SearchIntent, SearchResultItem, SyncStatusState, TagComboboxIntent, TextIntent,
};
use super::tag_combobox::{tag_combobox_completion, tag_combobox_matches};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchKind {
    Navigate,
    AddDependency,
    AddRelated,
    AddEpicChild { display_ref: String },
}

impl From<&SearchIntent> for SearchKind {
    fn from(intent: &SearchIntent) -> Self {
        match intent {
            SearchIntent::Navigate => Self::Navigate,
            SearchIntent::AddDependency { .. } => Self::AddDependency,
            SearchIntent::AddRelated { .. } => Self::AddRelated,
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
            Self::AddRelated => "Add related task".to_string(),
            Self::AddEpicChild { display_ref } => format!("Add child to {display_ref}"),
        }
    }

    pub(crate) fn enter_hint(&self) -> &'static str {
        match self {
            Self::Navigate => "open task",
            Self::AddDependency => "add selected as blocker",
            Self::AddRelated => "add selected as related",
            Self::AddEpicChild { .. } => "add selected child",
        }
    }

    pub(crate) fn placeholder(&self) -> &'static str {
        match self {
            Self::Navigate => "Search tasks, notes, labels, and projects...",
            Self::AddDependency => "Search for the task that blocks this task...",
            Self::AddRelated => "Search for a related task...",
            Self::AddEpicChild { .. } => "Search for an existing task or create a child...",
        }
    }

    pub(crate) fn tab_hint(&self) -> Option<&'static str> {
        match self {
            Self::Navigate => Some("open results"),
            Self::AddDependency | Self::AddRelated | Self::AddEpicChild { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayView<'a> {
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
        results: &'a [SearchResultItem],
        selected: usize,
        total_matches: usize,
        stale: bool,
        no_matches_cached: bool,
        intent: SearchKind,
    },
    Command {
        input: String,
        cursor: usize,
        session: &'a crate::tui::event::CommandSessionSnapshot,
        catalog: &'a crate::tui::event::CommandCatalog,
        candidates: &'a [crate::tui::event::CommandCandidate],
        highlighted: Option<usize>,
    },
    AddTask(Box<AddTaskView<'a>>),
    TextInput(TextInputView),
    MultilineInput(MultilineInputView<'a>),
    Picker(PickerView<'a>),
    TagCombobox(Box<TagComboboxView<'a>>),
    HeaderMenu(HeaderMenuView<'a>),
    OrderMenu(OrderMenuView),
    Confirm(ConfirmView),
    TextPanel(TextPanelView<'a>),
    Changelog {
        markdown: &'a str,
        scroll: u16,
    },
    RecurrenceHistory(RecurrenceHistoryView<'a>),
    SyncStatus(Box<SyncStatusView<'a>>),
    DatabaseStats {
        stats: &'a TuiDatabaseStats,
        scroll: u16,
    },
    Update(&'a super::state::UpdateOverlayState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncStatusView<'a> {
    pub(crate) state: SyncStatusState,
    pub(crate) status: &'a TuiSyncStatus,
    pub(crate) syncing: bool,
    pub(crate) now: time::OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelView<'a> {
    pub(crate) title: String,
    pub(crate) lines: &'a [String],
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskAttachmentsView<'a> {
    pub(crate) items: &'a [PendingTaskAttachmentSummary],
    pub(crate) selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecurrenceHistoryView<'a> {
    pub(crate) page: &'a aven_core::query::RecurrenceHistoryPage,
    pub(crate) selected: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskView<'a> {
    pub(crate) title: String,
    pub(crate) title_cursor: usize,
    pub(crate) description: &'a [String],
    pub(crate) description_row: usize,
    pub(crate) description_column: usize,
    pub(crate) focus: AddTaskStep,
    pub(crate) project: String,
    pub(crate) status: String,
    pub(crate) status_automatic: bool,
    pub(crate) priority: String,
    pub(crate) labels: &'a [String],
    pub(crate) is_epic: bool,
    pub(crate) create_more: bool,
    pub(crate) create_more_available: bool,
    pub(crate) available_at: String,
    pub(crate) available_at_cursor: usize,
    pub(crate) due_on: String,
    pub(crate) due_on_cursor: usize,
    pub(crate) schedule_input: String,
    pub(crate) schedule_input_cursor: usize,
    pub(crate) schedule_error: Option<String>,
    pub(crate) schedule_validation_requested: bool,
    pub(crate) attachments: Box<AddTaskAttachmentsView<'a>>,
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
    pub(crate) recurrence_preview: &'a [String],
    pub(crate) recurrence_error: Option<String>,
    pub(crate) mode: &'a AddTaskMode,
    pub(crate) title_error: bool,
    pub(crate) status_prefix_active: bool,
    pub(crate) priority_prefix_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextInputKind {
    AddProject,
    ProjectPath,
    AddLabel,
    RenameLabel,
    ConfirmDeleteLabel,
    AddWorkspace,
    RenameWorkspace,
    RenameProject,
    ConfirmDeleteProject,
    EditTitle,
    EditDate,
    SaveAttachment,
    ConflictManual,
}

impl From<&TextIntent> for TextInputKind {
    fn from(intent: &TextIntent) -> Self {
        match intent {
            TextIntent::AddProject => Self::AddProject,
            TextIntent::AddProjectPath { .. } => Self::ProjectPath,
            TextIntent::AddLabel => Self::AddLabel,
            TextIntent::RenameLabel { .. } => Self::RenameLabel,
            TextIntent::ConfirmDeleteLabel { .. } => Self::ConfirmDeleteLabel,
            TextIntent::AddWorkspace => Self::AddWorkspace,
            TextIntent::RenameWorkspace { .. } => Self::RenameWorkspace,
            TextIntent::RenameProject { .. } => Self::RenameProject,
            TextIntent::ConfirmDeleteProject { .. } => Self::ConfirmDeleteProject,
            TextIntent::EditTitle { .. } => Self::EditTitle,
            TextIntent::EditAvailability { .. } | TextIntent::EditDue { .. } => Self::EditDate,
            TextIntent::SaveAttachment { .. } => Self::SaveAttachment,
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
pub(crate) struct MultilineInputView<'a> {
    pub(crate) kind: MultilineInputKind,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: &'a [String],
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
    RenameProject,
    DeleteProject,
    EditPriority,
    LabelAdministration,
    SwitchWorkspace,
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
            PickerIntent::RenameProject => Self::RenameProject,
            PickerIntent::DeleteProject => Self::DeleteProject,
            PickerIntent::EditPriority { .. } => Self::EditPriority,
            PickerIntent::BrowseLabels | PickerIntent::RenameLabel | PickerIntent::DeleteLabel => {
                Self::LabelAdministration
            }
            PickerIntent::SwitchWorkspace => Self::SwitchWorkspace,
            _ => Self::Generic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerView<'a> {
    pub(crate) kind: PickerKind,
    pub(crate) title: String,
    pub(crate) filter: String,
    pub(crate) filter_cursor: usize,
    pub(crate) items: &'a [PickerItem],
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
pub(crate) struct TagComboboxView<'a> {
    pub(crate) kind: TagComboboxKind,
    pub(crate) title: String,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) completion: Option<String>,
    pub(crate) options: &'a [String],
    pub(crate) selected: &'a [String],
    pub(crate) partial: &'a [String],
    pub(crate) highlighted: usize,
    pub(crate) visible_indices: Vec<usize>,
    pub(crate) visible_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderMenuView<'a> {
    pub(crate) kind: HeaderMenuKind,
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) selected: usize,
    pub(crate) items: &'a [HeaderMenuItem],
}

impl<'a> From<&'a HeaderMenuState> for HeaderMenuView<'a> {
    fn from(state: &'a HeaderMenuState) -> Self {
        Self {
            kind: state.kind,
            column: state.column,
            row: state.row,
            selected: state.selected,
            items: &state.items,
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

impl TextInputView {
    pub(crate) fn from_state(state: &super::state::TextInputState) -> Self {
        Self {
            kind: (&state.intent).into(),
            title: state.title.clone(),
            prompt: state.prompt.clone(),
            input: state.input.as_str().to_string(),
            cursor: state.input.cursor,
        }
    }
}

impl<'a> MultilineInputView<'a> {
    pub(crate) fn from_state(state: &'a super::state::MultilineInputState) -> Self {
        Self {
            kind: (&state.intent).into(),
            title: state.title.clone(),
            prompt: state.prompt.clone(),
            lines: &state.lines,
            row: state.row,
            column: state.column,
            mode: state.mode,
        }
    }
}

impl<'a> PickerView<'a> {
    pub(crate) fn from_state(state: &'a super::state::PickerState) -> Self {
        Self {
            kind: (&state.intent).into(),
            title: state.title.clone(),
            filter: state.filter.as_str().to_string(),
            filter_cursor: state.filter.cursor,
            items: &state.items,
            selected: state.selected,
            scroll: state.scroll,
            multi: state.multi,
            mode: state.mode,
            visible_indices: visible_picker_indices(state),
        }
    }
}

impl<'a> TagComboboxView<'a> {
    pub(crate) fn from_state(state: &'a super::state::TagComboboxState) -> Self {
        let visible_indices = tag_combobox_matches(state);
        Self {
            kind: (&state.intent).into(),
            title: state.title.clone(),
            input: state.input.as_str().to_string(),
            input_cursor: state.input.cursor,
            completion: tag_combobox_completion(state),
            options: &state.options,
            selected: &state.selected,
            partial: &state.partial,
            highlighted: state.highlighted,
            visible_start: visible_indices
                .iter()
                .position(|index| *index == state.highlighted)
                .unwrap_or(0)
                .saturating_sub(TAG_COMBOBOX_VIEWPORT_ROWS.saturating_sub(1)),
            visible_indices,
        }
    }
}

impl<'a> RecurrenceHistoryView<'a> {
    pub(crate) fn from_state(state: &'a super::state::RecurrenceHistoryState) -> Self {
        Self {
            page: &state.page,
            selected: state.selected,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct OverlayViewContext<'a> {
    pub(crate) sync_status: &'a TuiSyncStatus,
    pub(crate) syncing: bool,
    pub(crate) now: time::OffsetDateTime,
    pub(crate) status_prefix_active: bool,
    pub(crate) priority_prefix_active: bool,
}

impl<'a> OverlayView<'a> {
    pub(crate) fn project(state: &'a OverlayState, context: OverlayViewContext<'a>) -> Self {
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
                input: state.input.as_str().to_string(),
                cursor: state.input.cursor,
                results: &state.results,
                selected: state.selected,
                total_matches: state.total_matches,
                stale: !state.results_are_current(),
                no_matches_cached: state.results_query.is_some()
                    && state.results.is_empty()
                    && state.total_matches == 0,
                intent: (&state.intent).into(),
            },
            Command { state } => Self::Command {
                input: state.input.as_str().to_string(),
                cursor: state.input.cursor,
                session: &state.session,
                catalog: &state.catalog,
                candidates: &state.candidates,
                highlighted: state.highlighted,
            },
            AddTask(state) => Self::AddTask(Box::new(AddTaskView {
                title: state.title.as_str().to_string(),
                title_cursor: state.title.cursor,
                description: &state.description.lines,
                description_row: state.description.row,
                description_column: state.description.column,
                focus: state.focus,
                project: state.project.clone(),
                status: state.effective_status().to_string(),
                status_automatic: state.status_is_automatic(),
                priority: state.priority.value().to_string(),
                labels: &state.labels,
                is_epic: state.is_epic,
                create_more: state.create_more,
                create_more_available: state.create_more_available && !state.recurrence_enabled(),
                available_at: state.available_at.as_str().to_string(),
                available_at_cursor: state.available_at.cursor,
                due_on: state.due_on.as_str().to_string(),
                due_on_cursor: state.due_on.cursor,
                schedule_input: state.schedule_input.as_str().to_string(),
                schedule_input_cursor: state.schedule_input.cursor,
                schedule_error: state.schedule_error.clone(),
                schedule_validation_requested: state.schedule_validation_requested,
                attachments: Box::new(AddTaskAttachmentsView {
                    items: &state.attachments,
                    selected: state.selected_attachment,
                }),
                recurrence_series_id: state.recurrence_series_id.clone(),
                editing_template: state.template_schedule.is_some(),
                repeat_rule: state.repeat_rule.as_str().to_string(),
                repeat_rule_cursor: state.repeat_rule.cursor,
                repeat_at: state.repeat_at.as_str().to_string(),
                repeat_at_cursor: state.repeat_at.cursor,
                repeat_due: state.repeat_due.clone(),
                time_zone: state.time_zone.clone(),
                repeat_start_on: state.repeat_start_on.as_str().to_string(),
                repeat_start_on_cursor: state.repeat_start_on.cursor,
                schedule_expanded: state.schedule_expanded,
                recurrence_preview: &state.recurrence_preview,
                recurrence_error: state.recurrence_error.clone(),
                mode: &state.mode,
                title_error: state.title_error,
                status_prefix_active: context.status_prefix_active,
                priority_prefix_active: context.priority_prefix_active,
            })),
            TextInput(state) => Self::TextInput(TextInputView::from_state(state)),
            MultilineInput(state) => Self::MultilineInput(MultilineInputView::from_state(state)),
            Picker(state) => Self::Picker(PickerView::from_state(state)),
            TagCombobox(state) => Self::TagCombobox(Box::new(TagComboboxView::from_state(state))),
            HeaderMenu(state) => Self::HeaderMenu(HeaderMenuView::from(state)),
            OrderMenu(state) => Self::OrderMenu(OrderMenuView::from(state)),
            Confirm(state) => Self::Confirm(ConfirmView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
            }),
            TextPanel(state) => Self::TextPanel(TextPanelView {
                title: state.title.clone(),
                lines: &state.lines,
                scroll: state.scroll,
            }),
            Changelog(state) => Self::Changelog {
                markdown: &state.markdown,
                scroll: state.scroll,
            },
            RecurrenceHistory(state) => {
                Self::RecurrenceHistory(RecurrenceHistoryView::from_state(state))
            }
            SyncStatus(state) => Self::SyncStatus(Box::new(SyncStatusView {
                state: state.clone(),
                status: context.sync_status,
                syncing: context.syncing,
                now: context.now,
            })),
            DatabaseStats { stats, scroll } => Self::DatabaseStats {
                stats,
                scroll: *scroll,
            },
            Update(state) => Self::Update(state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::{LineEdit, PickerIntent, PickerState};

    #[test]
    fn overlay_view_projection_keeps_project_picker_presentation_kinds() {
        for (intent, expected_kind) in [
            (PickerIntent::RenameProject, PickerKind::RenameProject),
            (PickerIntent::DeleteProject, PickerKind::DeleteProject),
        ] {
            let state = OverlayState::Picker(PickerState {
                intent,
                title: "Manage project".to_string(),
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
            });
            let sync_status = TuiSyncStatus::default();
            let picker = OverlayView::project(
                &state,
                OverlayViewContext {
                    sync_status: &sync_status,
                    syncing: false,
                    now: time::OffsetDateTime::UNIX_EPOCH,
                    status_prefix_active: false,
                    priority_prefix_active: false,
                },
            );
            assert!(matches!(
                picker,
                OverlayView::Picker(PickerView { kind, .. }) if kind == expected_kind
            ));
        }
    }
}

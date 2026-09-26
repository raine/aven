use crate::ids::WorkspaceId;
use crate::tui::overlay::text_input::LineEdit;
use crate::tui::store::{TaskOrder, TaskQuery, TuiDatabaseStats};
use aven_core::query::{RecurrenceHistoryEntry, RecurrenceHistoryPage};
use aven_core::recurrence::RecurrenceSeriesId;
use chrono::{DateTime, Utc};
use unicode_width::UnicodeWidthStr;

mod authoring;
mod command_search;
mod editors;

pub(crate) use authoring::{
    AddTaskMode, AddTaskState, ScheduleEditorField, ScheduleEditorMode, ScheduleEditorState,
};
pub(crate) use command_search::{
    CommandAvailabilityOverride, CommandState, SearchIntent, SearchResultItem, SearchState,
};
pub(crate) use editors::{
    ConfirmIntent, ConfirmState, EpicChildRemovalRestoration, MultilineInputMode,
    MultilineInputState, MultilineIntent, OverlayTarget, PickerIntent, PickerItem, PickerMode,
    PickerState, TagComboboxIntent, TagComboboxState, TextInputState, TextIntent,
};

/// The Sync › Add device page: the invitation is created behind a loading
/// state, then its QR code replaces it in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairingOverlay {
    Creating { started_at: std::time::Instant },
    Failed(String),
    Ready(std::sync::Arc<crate::pairing::PairingPresentation>),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayState {
    Metadata(Box<super::metadata::MetadataState>),
    Onboarding {
        persist_on_exit: bool,
    },
    Help {
        scroll: u16,
    },
    Detail,
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
    Changelog(ChangelogState),
    Pairing(PairingOverlay),
    RecurrenceHistory(Box<RecurrenceHistoryState>),
    Sync(super::sync_dialog::SyncDialogState),
    DatabaseStats {
        stats: Box<TuiDatabaseStats>,
        scroll: u16,
    },
    Update(UpdateOverlayState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangelogState {
    pub(crate) markdown: String,
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelState {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
}

pub(crate) const RECURRENCE_HISTORY_PAGE_SIZE: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RecurrenceHistoryEntryKey {
    Slot(String),
    PauseStartedAt(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecurrenceHistoryAction {
    OpenTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecurrenceHistoryState {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) series_id: RecurrenceSeriesId,
    pub(crate) as_of: DateTime<Utc>,
    pub(crate) page: RecurrenceHistoryPage,
    pub(crate) selected: Option<usize>,
}

impl RecurrenceHistoryState {
    pub(crate) fn new(
        workspace_id: WorkspaceId,
        series_id: RecurrenceSeriesId,
        as_of: DateTime<Utc>,
        page: RecurrenceHistoryPage,
    ) -> Self {
        let selected = (!page.items.is_empty()).then_some(0);
        Self {
            workspace_id,
            series_id,
            as_of,
            page,
            selected,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub(crate) fn selected_entry(&self) -> Option<&RecurrenceHistoryEntry> {
        self.selected.and_then(|index| self.page.items.get(index))
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if self.page.items.is_empty() {
            self.selected = None;
            return;
        }
        let Some(current) = self.selected else {
            self.selected = Some(0);
            return;
        };
        let last = self.page.items.len().saturating_sub(1);
        self.selected = Some(current.saturating_add_signed(delta).min(last));
    }

    pub(crate) fn replace_page(
        &mut self,
        page: RecurrenceHistoryPage,
        preferred: Option<RecurrenceHistoryEntryKey>,
        fallback_index: usize,
    ) {
        self.page = page;
        self.selected = preferred
            .and_then(|key| {
                self.page
                    .items
                    .iter()
                    .position(|entry| recurrence_history_entry_key(entry) == key)
            })
            .or_else(|| {
                (!self.page.items.is_empty())
                    .then(|| fallback_index.min(self.page.items.len().saturating_sub(1)))
            });
    }
}

fn recurrence_history_entry_key(entry: &RecurrenceHistoryEntry) -> RecurrenceHistoryEntryKey {
    match (entry.slot_on.as_ref(), entry.interval_started_at.as_ref()) {
        (Some(slot), None) => RecurrenceHistoryEntryKey::Slot(slot.clone()),
        (None, Some(started_at)) => RecurrenceHistoryEntryKey::PauseStartedAt(started_at.clone()),
        _ => panic!("history entry must identify one slot or pause interval"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateNotesState {
    Loading,
    Ready(String),
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateActionFocus {
    Later,
    Primary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateOverlayState {
    Checking,
    Available {
        plan: crate::update::InstallPlan,
        notes: UpdateNotesState,
        scroll: u16,
        focus: UpdateActionFocus,
        cached: bool,
    },
    Progress {
        version: String,
        phase: crate::update::UpdatePhase,
        cancelling: bool,
    },
    Current {
        version: String,
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
    View(TaskQuery),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderMenuItem {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) selected: bool,
    pub(crate) action: HeaderMenuAction,
}

pub(crate) fn header_menu_area(
    kind: HeaderMenuKind,
    column: u16,
    row: u16,
    items: &[HeaderMenuItem],
    terminal_width: u16,
    terminal_height: u16,
) -> ratatui::layout::Rect {
    let width = header_menu_width(kind, items).min(terminal_width);
    let height = (items.len() as u16).saturating_add(2).min(terminal_height);
    let x = column.min(terminal_width.saturating_sub(width));
    let y = row
        .saturating_add(1)
        .min(terminal_height.saturating_sub(height));
    ratatui::layout::Rect {
        x,
        y,
        width,
        height,
    }
}

fn header_menu_width(kind: HeaderMenuKind, items: &[HeaderMenuItem]) -> u16 {
    let title_width = header_menu_title(kind).width() as u16;
    let item_width = items
        .iter()
        .map(HeaderMenuItem::line_width)
        .max()
        .unwrap_or(0);
    title_width.max(item_width).saturating_add(4).max(16)
}

fn header_menu_title(kind: HeaderMenuKind) -> &'static str {
    match kind {
        HeaderMenuKind::Workspace => "workspace",
        HeaderMenuKind::Scope => "scope",
        HeaderMenuKind::View => "view",
    }
}

impl HeaderMenuState {
    pub(crate) fn area(&self, terminal_width: u16, terminal_height: u16) -> ratatui::layout::Rect {
        header_menu_area(
            self.kind,
            self.column,
            self.row,
            &self.items,
            terminal_width,
            terminal_height,
        )
    }

    pub(crate) fn selected_action(&self) -> Option<HeaderMenuAction> {
        self.items
            .get(self.selected)
            .map(|item| item.action.clone())
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
pub(crate) enum OverlaySubmit {
    MetadataSave {
        state: Box<super::metadata::MetadataState>,
        remove: bool,
    },

    MetadataExternalEditor(Box<super::metadata::MetadataState>),
    AddTask(Box<AddTaskState>),
    CreateAddTaskProject {
        state: Box<AddTaskState>,
        name: String,
    },
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

    pub(crate) fn multiline_input_with_baseline(
        intent: MultilineIntent,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
        baseline: String,
    ) -> Self {
        Self::MultilineInput(MultilineInputState::from_value_with_baseline(
            intent, title, prompt, value, baseline,
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
    use aven_core::query::RecurrenceHistoryKind;

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
        });
        assert!(matches!(
            search.intent,
            SearchIntent::AddEpicChild {
                ref display_ref,
                ref project_key,
                    ..
            } if display_ref == "APP-1234" && project_key == "app"
        ));

        let confirm = ConfirmState::new(
            ConfirmIntent::DeleteAttachment {
                attachment_id: "attachment-1".to_string(),
            },
            "Delete attachment",
            "Delete attachment?",
        );
        assert!(matches!(
            confirm.intent,
            ConfirmIntent::DeleteAttachment {
                ref attachment_id,
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
        assert_eq!(
            multiline.buffer.lines,
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn recurrence_history_selection_uses_resident_indexes_and_reload_identity() {
        let workspace_id = WorkspaceId::new();
        let series_id = RecurrenceSeriesId::new();
        let page = RecurrenceHistoryPage {
            series_ref: "RCR-TEST".to_string(),
            items: vec![history_entry("2026-07-22"), history_entry("2026-07-21")],
            offset: 0,
            limit: 10,
            total: 2,
            has_more: false,
        };
        let mut state = RecurrenceHistoryState::new(workspace_id, series_id, Utc::now(), page);

        assert_eq!(state.selected_index(), Some(0));
        state.move_selection(1);
        assert_eq!(state.selected_index(), Some(1));
        let selected = state.selected_entry().map(recurrence_history_entry_key);
        state.replace_page(
            RecurrenceHistoryPage {
                series_ref: "RCR-TEST".to_string(),
                items: vec![history_entry("2026-07-21"), history_entry("2026-07-20")],
                offset: 1,
                limit: 10,
                total: 2,
                has_more: false,
            },
            selected,
            0,
        );
        assert_eq!(state.selected_index(), Some(0));
    }

    fn history_entry(slot_on: &str) -> RecurrenceHistoryEntry {
        RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Missed,
            slot_on: Some(slot_on.to_string()),
            interval_started_at: None,
            interval_ended_at: None,
            task_id: None,
            task_ref: None,
            openable: false,
            archived_projection: false,
            resolved_at: None,
        }
    }
}

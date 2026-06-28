use crate::query::SearchMatchedField;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::text_input::LineEdit;
use crate::tui::store::{TaskOrder, TaskView, TuiDatabaseStats, TuiSyncStatus};
use crate::tui::text::{char_boundary_at_or_before, normalize_pasted_newlines};
use unicode_width::UnicodeWidthStr;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayState {
    Help {
        scroll: u16,
    },
    Detail {
        scroll: u16,
    },
    DetailHelp {
        scroll: u16,
    },
    Search(SearchState),
    Command {
        state: CommandState,
    },
    AddTask(AddTaskState),
    TextInput(TextInputState),
    MultilineInput(MultilineInputState),
    Picker(PickerState),
    TagCombobox(TagComboboxState),
    HeaderMenu(HeaderMenuState),
    OrderMenu(OrderMenuState),
    Confirm(ConfirmState),
    TextPanel(TextPanelState),
    SyncStatus(Box<TuiSyncStatus>),
    DatabaseStats {
        stats: Box<TuiDatabaseStats>,
        scroll: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchResultItem {
    pub(crate) task_id: String,
    pub(crate) display_ref: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project_key: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) created_at: String,
    pub(crate) labels: Vec<String>,
    pub(crate) matched_field: SearchMatchedField,
    pub(crate) snippet: Option<String>,
    pub(crate) score: i64,
    pub(crate) deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchState {
    pub(crate) input: LineEdit,
    pub(crate) results: Vec<SearchResultItem>,
    pub(crate) selected: usize,
    pub(crate) total_matches: usize,
    pub(crate) results_query: Option<String>,
}

impl SearchState {
    pub(crate) fn blank() -> Self {
        Self {
            input: LineEdit::blank(),
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            results_query: None,
        }
    }

    pub(crate) fn current_query(&self) -> String {
        self.input.text.trim().to_string()
    }

    pub(crate) fn clear_results(&mut self) {
        self.results.clear();
        self.selected = 0;
        self.total_matches = 0;
        self.results_query = None;
    }

    pub(crate) fn selected_result(&self) -> Option<&SearchResultItem> {
        self.results.get(self.selected)
    }

    pub(crate) fn results_are_current(&self) -> bool {
        self.results_query.as_deref() == Some(self.input.text.trim())
    }

    pub(crate) fn selected_current_result(&self) -> Option<&SearchResultItem> {
        self.results_are_current()
            .then(|| self.selected_result())
            .flatten()
    }

    pub(crate) fn normalize_selection(&mut self) {
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.results.len() - 1);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandState {
    pub(crate) input: LineEdit,
    pub(crate) cycle_input: Option<String>,
    pub(crate) cycle_index: usize,
    pub(crate) highlighted: Option<String>,
}

impl CommandState {
    pub(crate) fn blank() -> Self {
        Self {
            input: LineEdit::blank(),
            cycle_input: None,
            cycle_index: 0,
            highlighted: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new(input: LineEdit) -> Self {
        Self {
            input,
            cycle_input: None,
            cycle_index: 0,
            highlighted: None,
        }
    }

    pub(crate) fn reset_cycle(&mut self) {
        self.cycle_input = None;
        self.cycle_index = 0;
        self.highlighted = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelState {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
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
    Status,
    Priority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeaderMenuAction {
    Workspace(String),
    WorkspaceScope,
    ProjectScope(String),
    View(TaskView),
    Status(String),
    Priority(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderMenuItem {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) selected: bool,
    pub(crate) action: HeaderMenuAction,
}

impl HeaderMenuState {
    pub(crate) fn area(&self, terminal_width: u16, terminal_height: u16) -> ratatui::layout::Rect {
        let width = self.width().min(terminal_width);
        let height = (self.items.len() as u16)
            .saturating_add(2)
            .min(terminal_height);
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

    pub(crate) fn selected_action(&self) -> Option<HeaderMenuAction> {
        self.items
            .get(self.selected)
            .map(|item| item.action.clone())
    }

    fn width(&self) -> u16 {
        let title_width = self.title().width() as u16;
        let item_width = self
            .items
            .iter()
            .map(|item| item.line_width())
            .max()
            .unwrap_or(0);
        title_width.max(item_width).saturating_add(4).max(16)
    }

    fn title(&self) -> &'static str {
        match self.kind {
            HeaderMenuKind::Workspace => "workspace",
            HeaderMenuKind::Scope => "scope",
            HeaderMenuKind::View => "view",
            HeaderMenuKind::Status => "status",
            HeaderMenuKind::Priority => "priority",
        }
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

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayRoute {
    MessageOnly,
    AddTaskTitle,
    AddTaskDescription,
    AddTaskNatural,
    AddTaskTitleProject,
    AddTaskTitlePriority,
    AddNote,
    AddProject,
    AddLabel,
    EditStatus,
    EditTitle,
    EditDescription,
    EditProject,
    EditPriority,
    EditLabels,
    FilterLabel,
    FilterPriority,
    ScopeProject,
    RenameProjectPicker,
    RenameProjectName,
    DeleteProjectPicker,
    DeleteProjectNameConfirm,
    DeleteProjectConfirm,
    DeleteTaskConfirm,
    SwitchWorkspace,
    ConflictField,
    ConflictConfirm,
    ConflictManual,
    ConfigInit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextSubmitRoute {
    AddTaskTitleToast,
    AddProject,
    AddLabel,
    RenameProjectName,
    DeleteProjectNameConfirm,
    EditTitle,
    ConflictManual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MultilineSubmitRoute {
    AddTaskDescription,
    AddTaskNatural,
    AddNote,
    EditDescription,
    ConflictManual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerSubmitRoute {
    AddTaskTitleProject,
    AddTaskTitlePriority,
    EditStatus,
    EditProject,
    EditPriority,
    EditLabels,
    FilterLabel,
    FilterPriority,
    ScopeProject,
    RenameProjectPicker,
    DeleteProjectPicker,
    SwitchWorkspace,
    ConflictField,
    ConflictManual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmSubmitRoute {
    ConflictConfirm,
    ConfigInit,
    DeleteProjectConfirm,
    DeleteTaskConfirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlaySubmitKind {
    Text,
    Multiline,
    Picker,
    Confirm,
}

impl OverlaySubmitKind {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 4] = [Self::Text, Self::Multiline, Self::Picker, Self::Confirm];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverlayFallbackMessages {
    pub(crate) text: &'static str,
    pub(crate) multiline: &'static str,
    pub(crate) picker: &'static str,
    pub(crate) confirm: &'static str,
}

impl Default for OverlayFallbackMessages {
    fn default() -> Self {
        Self {
            text: "submitted overlay",
            multiline: "submitted overlay",
            picker: "selected overlay",
            confirm: "confirmed overlay",
        }
    }
}

impl OverlayFallbackMessages {
    pub(crate) fn message(self, kind: OverlaySubmitKind) -> &'static str {
        match kind {
            OverlaySubmitKind::Text => self.text,
            OverlaySubmitKind::Multiline => self.multiline,
            OverlaySubmitKind::Picker => self.picker,
            OverlaySubmitKind::Confirm => self.confirm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverlayRouteDescriptor {
    pub(crate) text_submit: Option<TextSubmitRoute>,
    pub(crate) multiline_submit: Option<MultilineSubmitRoute>,
    pub(crate) picker_submit: Option<PickerSubmitRoute>,
    pub(crate) confirm_submit: Option<ConfirmSubmitRoute>,
    pub(crate) initial_picker_mode: PickerMode,
    pub(crate) fallback: OverlayFallbackMessages,
}

impl Default for OverlayRouteDescriptor {
    fn default() -> Self {
        Self {
            text_submit: None,
            multiline_submit: None,
            picker_submit: None,
            confirm_submit: None,
            initial_picker_mode: PickerMode::Navigate,
            fallback: OverlayFallbackMessages::default(),
        }
    }
}

impl OverlayRoute {
    pub(crate) fn descriptor(self) -> OverlayRouteDescriptor {
        match self {
            Self::MessageOnly => OverlayRouteDescriptor {
                fallback: OverlayFallbackMessages {
                    text: "submitted overlay",
                    multiline: "submitted overlay",
                    picker: "selected overlay",
                    confirm: "confirmed overlay",
                },
                ..OverlayRouteDescriptor::default()
            },
            Self::AddTaskTitle => OverlayRouteDescriptor {
                text_submit: Some(TextSubmitRoute::AddTaskTitleToast),
                ..OverlayRouteDescriptor::default()
            },
            Self::AddTaskDescription => OverlayRouteDescriptor {
                multiline_submit: Some(MultilineSubmitRoute::AddTaskDescription),
                ..OverlayRouteDescriptor::default()
            },
            Self::AddTaskNatural => OverlayRouteDescriptor {
                multiline_submit: Some(MultilineSubmitRoute::AddTaskNatural),
                ..OverlayRouteDescriptor::default()
            },
            Self::AddTaskTitleProject => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::AddTaskTitleProject),
                initial_picker_mode: PickerMode::Filter,
                ..OverlayRouteDescriptor::default()
            },
            Self::AddTaskTitlePriority => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::AddTaskTitlePriority),
                ..OverlayRouteDescriptor::default()
            },
            Self::AddNote => OverlayRouteDescriptor {
                multiline_submit: Some(MultilineSubmitRoute::AddNote),
                ..OverlayRouteDescriptor::default()
            },
            Self::AddProject => OverlayRouteDescriptor {
                text_submit: Some(TextSubmitRoute::AddProject),
                ..OverlayRouteDescriptor::default()
            },
            Self::AddLabel => OverlayRouteDescriptor {
                text_submit: Some(TextSubmitRoute::AddLabel),
                ..OverlayRouteDescriptor::default()
            },
            Self::EditStatus => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::EditStatus),
                ..OverlayRouteDescriptor::default()
            },
            Self::EditTitle => OverlayRouteDescriptor {
                text_submit: Some(TextSubmitRoute::EditTitle),
                ..OverlayRouteDescriptor::default()
            },
            Self::EditDescription => OverlayRouteDescriptor {
                multiline_submit: Some(MultilineSubmitRoute::EditDescription),
                ..OverlayRouteDescriptor::default()
            },
            Self::EditProject => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::EditProject),
                initial_picker_mode: PickerMode::Filter,
                ..OverlayRouteDescriptor::default()
            },
            Self::EditPriority => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::EditPriority),
                ..OverlayRouteDescriptor::default()
            },
            Self::EditLabels => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::EditLabels),
                ..OverlayRouteDescriptor::default()
            },
            Self::FilterLabel => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::FilterLabel),
                ..OverlayRouteDescriptor::default()
            },
            Self::FilterPriority => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::FilterPriority),
                ..OverlayRouteDescriptor::default()
            },
            Self::ScopeProject => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::ScopeProject),
                initial_picker_mode: PickerMode::Filter,
                ..OverlayRouteDescriptor::default()
            },
            Self::RenameProjectPicker => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::RenameProjectPicker),
                initial_picker_mode: PickerMode::Filter,
                ..OverlayRouteDescriptor::default()
            },
            Self::RenameProjectName => OverlayRouteDescriptor {
                text_submit: Some(TextSubmitRoute::RenameProjectName),
                ..OverlayRouteDescriptor::default()
            },
            Self::DeleteProjectPicker => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::DeleteProjectPicker),
                initial_picker_mode: PickerMode::Filter,
                ..OverlayRouteDescriptor::default()
            },
            Self::DeleteProjectNameConfirm => OverlayRouteDescriptor {
                text_submit: Some(TextSubmitRoute::DeleteProjectNameConfirm),
                ..OverlayRouteDescriptor::default()
            },
            Self::DeleteProjectConfirm => OverlayRouteDescriptor {
                confirm_submit: Some(ConfirmSubmitRoute::DeleteProjectConfirm),
                ..OverlayRouteDescriptor::default()
            },
            Self::DeleteTaskConfirm => OverlayRouteDescriptor {
                confirm_submit: Some(ConfirmSubmitRoute::DeleteTaskConfirm),
                ..OverlayRouteDescriptor::default()
            },
            Self::SwitchWorkspace => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::SwitchWorkspace),
                ..OverlayRouteDescriptor::default()
            },
            Self::ConflictField => OverlayRouteDescriptor {
                picker_submit: Some(PickerSubmitRoute::ConflictField),
                ..OverlayRouteDescriptor::default()
            },
            Self::ConflictConfirm => OverlayRouteDescriptor {
                confirm_submit: Some(ConfirmSubmitRoute::ConflictConfirm),
                ..OverlayRouteDescriptor::default()
            },
            Self::ConflictManual => OverlayRouteDescriptor {
                text_submit: Some(TextSubmitRoute::ConflictManual),
                multiline_submit: Some(MultilineSubmitRoute::ConflictManual),
                picker_submit: Some(PickerSubmitRoute::ConflictManual),
                ..OverlayRouteDescriptor::default()
            },
            Self::ConfigInit => OverlayRouteDescriptor {
                confirm_submit: Some(ConfirmSubmitRoute::ConfigInit),
                ..OverlayRouteDescriptor::default()
            },
        }
    }

    pub(crate) fn text_submit_route(self) -> Option<TextSubmitRoute> {
        self.descriptor().text_submit
    }

    pub(crate) fn multiline_submit_route(self) -> Option<MultilineSubmitRoute> {
        self.descriptor().multiline_submit
    }

    pub(crate) fn picker_submit_route(self) -> Option<PickerSubmitRoute> {
        self.descriptor().picker_submit
    }

    pub(crate) fn initial_picker_mode(self) -> PickerMode {
        self.descriptor().initial_picker_mode
    }

    pub(crate) fn confirm_submit_route(self) -> Option<ConfirmSubmitRoute> {
        self.descriptor().confirm_submit
    }

    pub(crate) fn fallback_message(self, kind: OverlaySubmitKind) -> &'static str {
        self.descriptor().fallback.message(kind)
    }
}

#[cfg(test)]
impl OverlayRoute {
    pub(crate) const ALL: [Self; 29] = [
        Self::MessageOnly,
        Self::AddTaskTitle,
        Self::AddTaskDescription,
        Self::AddTaskNatural,
        Self::AddTaskTitleProject,
        Self::AddTaskTitlePriority,
        Self::AddNote,
        Self::AddProject,
        Self::AddLabel,
        Self::EditStatus,
        Self::EditTitle,
        Self::EditDescription,
        Self::EditProject,
        Self::EditPriority,
        Self::EditLabels,
        Self::FilterLabel,
        Self::FilterPriority,
        Self::ScopeProject,
        Self::RenameProjectPicker,
        Self::RenameProjectName,
        Self::DeleteProjectPicker,
        Self::DeleteProjectNameConfirm,
        Self::DeleteProjectConfirm,
        Self::DeleteTaskConfirm,
        Self::SwitchWorkspace,
        Self::ConflictField,
        Self::ConflictConfirm,
        Self::ConflictManual,
        Self::ConfigInit,
    ];

    pub(crate) fn submit_kinds(self) -> Vec<OverlaySubmitKind> {
        let descriptor = self.descriptor();
        OverlaySubmitKind::ALL
            .iter()
            .copied()
            .filter(|kind| match kind {
                OverlaySubmitKind::Text => descriptor.text_submit.is_some(),
                OverlaySubmitKind::Multiline => descriptor.multiline_submit.is_some(),
                OverlaySubmitKind::Picker => descriptor.picker_submit.is_some(),
                OverlaySubmitKind::Confirm => descriptor.confirm_submit.is_some(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskState {
    pub(crate) title: LineEdit,
    pub(crate) description: MultilineInputState,
    pub(crate) focus: AddTaskStep,
    pub(crate) project: String,
    pub(crate) status: String,
    pub(crate) priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextInputState {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) input: LineEdit,
}

impl TextInputState {
    pub(crate) fn new(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
        input: String,
    ) -> Self {
        Self {
            route,
            title: title.into(),
            prompt: prompt.into(),
            input: LineEdit::new(input),
        }
    }

    pub(crate) fn blank(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::new(route, title, prompt, String::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultilineInputState {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
}

impl MultilineInputState {
    pub(crate) fn insert_paste(&mut self, text: &str) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let row = self.row.min(self.lines.len() - 1);
        let column = char_boundary_at_or_before(&self.lines[row], self.column);
        self.row = row;
        self.column = column;

        let text = normalize_pasted_newlines(text);
        let mut pasted_lines = text.split('\n');
        let first = pasted_lines.next().unwrap_or_default();
        let rest = self.lines[row].split_off(column);
        self.lines[row].push_str(first);

        let mut insert_at = row;
        for line in pasted_lines {
            insert_at += 1;
            self.lines.insert(insert_at, line.to_string());
        }
        self.lines[insert_at].push_str(&rest);
        self.row = insert_at;
        self.column = self.lines[insert_at].len().saturating_sub(rest.len());
    }

    pub(crate) fn blank(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            route,
            title: title.into(),
            prompt: prompt.into(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }
    }

    pub(crate) fn from_value(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
    ) -> Self {
        let mut lines = value.split('\n').map(str::to_string).collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let row = lines.len() - 1;
        let column = lines[row].len();
        Self {
            route,
            title: title.into(),
            prompt: prompt.into(),
            lines,
            row,
            column,
        }
    }
}

impl PickerState {
    pub(crate) fn new(
        route: OverlayRoute,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        multi: bool,
    ) -> Self {
        let selected = Self::selected_index(&items);
        Self {
            route,
            title: title.into(),
            filter: LineEdit::blank(),
            items,
            selected,
            scroll: 0,
            multi,
            mode: route.initial_picker_mode(),
        }
    }

    fn selected_index(items: &[PickerItem]) -> usize {
        items.iter().position(|item| item.selected).unwrap_or(0)
    }
}

impl ConfirmState {
    pub(crate) fn new(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            route,
            title: title.into(),
            prompt: prompt.into(),
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
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) filter: LineEdit,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
    pub(crate) multi: bool,
    pub(crate) mode: PickerMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagComboboxState {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) input: LineEdit,
    pub(crate) options: Vec<String>,
    pub(crate) selected: Vec<String>,
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
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlaySubmit {
    AddTask {
        title: String,
        description: String,
    },
    Text {
        route: OverlayRoute,
        value: String,
    },
    Multiline {
        route: OverlayRoute,
        value: String,
    },
    Picker {
        route: OverlayRoute,
        values: Vec<String>,
    },
    HeaderMenu {
        action: HeaderMenuAction,
    },
    Order {
        order: TaskOrder,
    },
    Confirm {
        route: OverlayRoute,
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
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
        input: String,
    ) -> Self {
        Self::TextInput(TextInputState::new(route, title, prompt, input))
    }

    pub(crate) fn blank_text_input(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::TextInput(TextInputState::blank(route, title, prompt))
    }

    pub(crate) fn multiline_input(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: String,
    ) -> Self {
        Self::MultilineInput(MultilineInputState::from_value(route, title, prompt, value))
    }

    pub(crate) fn blank_multiline_input(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::MultilineInput(MultilineInputState::blank(route, title, prompt))
    }

    pub(crate) fn picker(
        route: OverlayRoute,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        multi: bool,
    ) -> Self {
        Self::Picker(PickerState::new(route, title, items, multi))
    }

    pub(crate) fn tag_combobox(
        route: OverlayRoute,
        title: impl Into<String>,
        options: Vec<String>,
        selected: Vec<String>,
    ) -> Self {
        let highlighted = options
            .iter()
            .position(|label| selected.contains(label))
            .unwrap_or(0);
        Self::TagCombobox(TagComboboxState {
            route,
            title: title.into(),
            input: LineEdit::blank(),
            options,
            selected,
            highlighted,
        })
    }

    pub(crate) fn confirm(
        route: OverlayRoute,
        title: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::Confirm(ConfirmState::new(route, title, prompt))
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

    #[test]
    fn picker_builder_uses_first_selected_item() {
        let state = PickerState::new(
            OverlayRoute::EditLabels,
            "Labels",
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
            true,
        );

        assert_eq!(state.selected, 1);
        assert_eq!(state.filter, LineEdit::blank());
        assert_eq!(state.mode, PickerMode::Navigate);
        assert!(state.multi);
    }

    #[test]
    fn project_pickers_open_in_filter_mode() {
        for route in [
            OverlayRoute::AddTaskTitleProject,
            OverlayRoute::EditProject,
            OverlayRoute::ScopeProject,
            OverlayRoute::RenameProjectPicker,
            OverlayRoute::DeleteProjectPicker,
        ] {
            let state = PickerState::new(
                route,
                "Project",
                vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                false,
            );
            assert_eq!(state.mode, PickerMode::Filter);
        }
    }

    #[test]
    fn overlay_builders_preserve_text_multiline_and_confirm_metadata() {
        let OverlayState::TextInput(text) = OverlayState::text_input(
            OverlayRoute::EditTitle,
            "Edit title",
            "title:",
            "old".to_string(),
        ) else {
            panic!("expected text input");
        };
        assert_eq!(text.route, OverlayRoute::EditTitle);
        assert_eq!(text.title, "Edit title");
        assert_eq!(text.prompt, "title:");
        assert_eq!(text.input.as_str(), "old");

        let OverlayState::MultilineInput(multiline) = OverlayState::multiline_input(
            OverlayRoute::EditDescription,
            "Edit description",
            "body:",
            "a\nb".to_string(),
        ) else {
            panic!("expected multiline input");
        };
        assert_eq!(multiline.route, OverlayRoute::EditDescription);
        assert_eq!(multiline.title, "Edit description");
        assert_eq!(multiline.prompt, "body:");
        assert_eq!(multiline.lines, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(multiline.row, 1);
        assert_eq!(multiline.column, 1);

        let OverlayState::Confirm(confirm) =
            OverlayState::confirm(OverlayRoute::DeleteTaskConfirm, "Delete", "Sure?")
        else {
            panic!("expected confirm");
        };
        assert_eq!(confirm.route, OverlayRoute::DeleteTaskConfirm);
        assert_eq!(confirm.title, "Delete");
        assert_eq!(confirm.prompt, "Sure?");
    }

    #[test]
    fn message_only_fallback_preserves_submit_kind_verbs() {
        assert_eq!(
            OverlayRoute::MessageOnly.fallback_message(OverlaySubmitKind::Text),
            "submitted overlay"
        );
        assert_eq!(
            OverlayRoute::MessageOnly.fallback_message(OverlaySubmitKind::Multiline),
            "submitted overlay"
        );
        assert_eq!(
            OverlayRoute::MessageOnly.fallback_message(OverlaySubmitKind::Picker),
            "selected overlay"
        );
        assert_eq!(
            OverlayRoute::MessageOnly.fallback_message(OverlaySubmitKind::Confirm),
            "confirmed overlay"
        );
    }

    #[test]
    fn all_route_kind_fallback_messages_are_non_empty() {
        for route in OverlayRoute::ALL {
            for kind in OverlaySubmitKind::ALL {
                assert!(
                    !route.fallback_message(kind).is_empty(),
                    "{route:?} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn conflict_manual_supports_multiple_submit_kinds() {
        let descriptor = OverlayRoute::ConflictManual.descriptor();
        assert!(descriptor.text_submit.is_some());
        assert!(descriptor.multiline_submit.is_some());
        assert!(descriptor.picker_submit.is_some());
    }
}

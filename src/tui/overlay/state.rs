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
    Search {
        input: LineEdit,
    },
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

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlaySubmitKind {
    Text,
    Multiline,
    Picker,
    Confirm,
}

#[cfg(test)]
impl OverlaySubmitKind {
    pub(crate) const ALL: [Self; 4] = [Self::Text, Self::Multiline, Self::Picker, Self::Confirm];
}

impl OverlayRoute {
    pub(crate) fn text_submit_route(self) -> Option<TextSubmitRoute> {
        match self {
            Self::AddTaskTitle => Some(TextSubmitRoute::AddTaskTitleToast),
            Self::AddProject => Some(TextSubmitRoute::AddProject),
            Self::AddLabel => Some(TextSubmitRoute::AddLabel),
            Self::RenameProjectName => Some(TextSubmitRoute::RenameProjectName),
            Self::DeleteProjectNameConfirm => Some(TextSubmitRoute::DeleteProjectNameConfirm),
            Self::EditTitle => Some(TextSubmitRoute::EditTitle),
            Self::ConflictManual => Some(TextSubmitRoute::ConflictManual),
            _ => None,
        }
    }

    pub(crate) fn multiline_submit_route(self) -> Option<MultilineSubmitRoute> {
        match self {
            Self::AddTaskDescription => Some(MultilineSubmitRoute::AddTaskDescription),
            Self::AddTaskNatural => Some(MultilineSubmitRoute::AddTaskNatural),
            Self::AddNote => Some(MultilineSubmitRoute::AddNote),
            Self::EditDescription => Some(MultilineSubmitRoute::EditDescription),
            Self::ConflictManual => Some(MultilineSubmitRoute::ConflictManual),
            _ => None,
        }
    }

    pub(crate) fn picker_submit_route(self) -> Option<PickerSubmitRoute> {
        match self {
            Self::AddTaskTitleProject => Some(PickerSubmitRoute::AddTaskTitleProject),
            Self::AddTaskTitlePriority => Some(PickerSubmitRoute::AddTaskTitlePriority),
            Self::EditStatus => Some(PickerSubmitRoute::EditStatus),
            Self::EditProject => Some(PickerSubmitRoute::EditProject),
            Self::EditPriority => Some(PickerSubmitRoute::EditPriority),
            Self::EditLabels => Some(PickerSubmitRoute::EditLabels),
            Self::FilterLabel => Some(PickerSubmitRoute::FilterLabel),
            Self::FilterPriority => Some(PickerSubmitRoute::FilterPriority),
            Self::ScopeProject => Some(PickerSubmitRoute::ScopeProject),
            Self::RenameProjectPicker => Some(PickerSubmitRoute::RenameProjectPicker),
            Self::DeleteProjectPicker => Some(PickerSubmitRoute::DeleteProjectPicker),
            Self::SwitchWorkspace => Some(PickerSubmitRoute::SwitchWorkspace),
            Self::ConflictField => Some(PickerSubmitRoute::ConflictField),
            Self::ConflictManual => Some(PickerSubmitRoute::ConflictManual),
            _ => None,
        }
    }

    pub(crate) fn initial_picker_mode(self) -> PickerMode {
        match self {
            Self::AddTaskTitleProject
            | Self::EditProject
            | Self::ScopeProject
            | Self::RenameProjectPicker
            | Self::DeleteProjectPicker => PickerMode::Filter,
            _ => PickerMode::Navigate,
        }
    }

    pub(crate) fn confirm_submit_route(self) -> Option<ConfirmSubmitRoute> {
        match self {
            Self::ConflictConfirm => Some(ConfirmSubmitRoute::ConflictConfirm),
            Self::ConfigInit => Some(ConfirmSubmitRoute::ConfigInit),
            Self::DeleteProjectConfirm => Some(ConfirmSubmitRoute::DeleteProjectConfirm),
            Self::DeleteTaskConfirm => Some(ConfirmSubmitRoute::DeleteTaskConfirm),
            _ => None,
        }
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
        OverlaySubmitKind::ALL
            .iter()
            .copied()
            .filter(|kind| match kind {
                OverlaySubmitKind::Text => self.text_submit_route().is_some(),
                OverlaySubmitKind::Multiline => self.multiline_submit_route().is_some(),
                OverlaySubmitKind::Picker => self.picker_submit_route().is_some(),
                OverlaySubmitKind::Confirm => self.confirm_submit_route().is_some(),
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
        title: String,
        value: String,
    },
    Multiline {
        route: OverlayRoute,
        title: String,
        value: String,
    },
    Picker {
        route: OverlayRoute,
        title: String,
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
        title: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayOutcome {
    None(OverlayState),
    Cancelled,
    Submitted(OverlaySubmit),
}

impl OverlaySubmit {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::AddTask { .. } => "submitted Add task".to_string(),
            Self::Text { title, .. } => format!("submitted {title}"),
            Self::Multiline { title, .. } => format!("submitted {title}"),
            Self::Picker { title, .. } => format!("selected {title}"),
            Self::HeaderMenu { .. } => "selected header menu".to_string(),
            Self::Order { order } => format!("selected order {order:?}"),
            Self::Confirm { title, .. } => format!("confirmed {title}"),
        }
    }
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
    fn picker_builder_defaults_to_first_item() {
        let state = PickerState::new(
            OverlayRoute::EditStatus,
            "Status",
            vec![PickerItem {
                label: "One".to_string(),
                value: "one".to_string(),
                selected: false,
            }],
            false,
        );

        assert_eq!(state.selected, 0);
        assert_eq!(state.filter, LineEdit::blank());
        assert_eq!(state.mode, PickerMode::Navigate);
        assert!(!state.multi);
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
}

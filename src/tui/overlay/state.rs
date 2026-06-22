use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::text_input::LineEdit;
use crate::tui::text::{char_boundary_at_or_before, normalize_pasted_newlines};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayState {
    Help { scroll: u16 },
    Detail { scroll: u16 },
    DetailHelp { scroll: u16 },
    Search { input: LineEdit },
    Command { input: LineEdit },
    AddTask(AddTaskState),
    TextInput(TextInputState),
    MultilineInput(MultilineInputState),
    Picker(PickerState),
    Confirm(ConfirmState),
    TextPanel(TextPanelState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelState {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
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
    FilterProject,
    FilterLabel,
    FilterStatus,
    FilterPriority,
    ViewProject,
    DeleteProjectPicker,
    DeleteProjectConfirm,
    DeleteTaskConfirm,
    SwitchWorkspace,
    ConflictField,
    ConflictConfirm,
    ConflictManual,
    ConfigInit,
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
impl OverlayRoute {
    pub(crate) fn submit_kinds(self) -> &'static [OverlaySubmitKind] {
        use OverlaySubmitKind::{Confirm, Multiline, Picker, Text};
        match self {
            Self::MessageOnly => &[],
            Self::AddTaskTitle => &[Text],
            Self::AddTaskDescription => &[Multiline],
            Self::AddTaskTitleProject | Self::AddTaskTitlePriority => &[Picker],
            Self::AddNote => &[Multiline],
            Self::AddProject | Self::AddLabel | Self::EditTitle => &[Text],
            Self::EditStatus | Self::EditProject | Self::EditPriority | Self::EditLabels => {
                &[Picker]
            }
            Self::EditDescription => &[Multiline],
            Self::FilterProject
            | Self::FilterLabel
            | Self::FilterStatus
            | Self::FilterPriority
            | Self::ViewProject
            | Self::DeleteProjectPicker
            | Self::SwitchWorkspace
            | Self::ConflictField => &[Picker],
            Self::DeleteProjectConfirm
            | Self::DeleteTaskConfirm
            | Self::ConflictConfirm
            | Self::ConfigInit => &[Confirm],
            Self::ConflictManual => &[Text, Multiline, Picker],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskState {
    pub(crate) title: LineEdit,
    pub(crate) description: MultilineInputState,
    pub(crate) focus: AddTaskStep,
    pub(crate) project: String,
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
            multi,
            mode: PickerMode::Navigate,
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
    pub(crate) multi: bool,
    pub(crate) mode: PickerMode,
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
}

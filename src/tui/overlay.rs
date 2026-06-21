use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayState {
    Help { scroll: u16 },
    Detail { scroll: u16 },
    DetailHelp { scroll: u16 },
    Search { input: LineEdit },
    Command { input: LineEdit },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineEdit {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

impl LineEdit {
    pub(crate) fn new(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }

    pub(crate) fn blank() -> Self {
        Self::new(String::new())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        let cursor = char_boundary_at_or_before(&self.text, self.cursor);
        match key.code {
            KeyCode::Left => self.cursor = previous_char_boundary(&self.text, cursor),
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = previous_char_boundary(&self.text, cursor);
            }
            KeyCode::Right => self.cursor = next_char_boundary(&self.text, cursor),
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = next_char_boundary(&self.text, cursor);
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
            }
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.text.len();
            }
            KeyCode::Backspace if cursor > 0 => {
                let previous = previous_char_boundary(&self.text, cursor);
                self.text.drain(previous..cursor);
                self.cursor = previous;
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) && cursor > 0 => {
                let previous = previous_char_boundary(&self.text, cursor);
                self.text.drain(previous..cursor);
                self.cursor = previous;
            }
            KeyCode::Delete if cursor < self.text.len() => {
                let next = next_char_boundary(&self.text, cursor);
                self.text.drain(cursor..next);
                self.cursor = cursor;
            }
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) && cursor < self.text.len() =>
            {
                let next = next_char_boundary(&self.text, cursor);
                self.text.drain(cursor..next);
                self.cursor = cursor;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.truncate(cursor);
                self.cursor = cursor;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.drain(..cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let previous = previous_word_start(&self.text, cursor);
                self.text.drain(previous..cursor);
                if previous > 0 && next_char_is_whitespace(&self.text, previous) {
                    let before = previous_char_boundary(&self.text, previous);
                    if self.text[before..previous].chars().all(char::is_whitespace) {
                        self.text.drain(before..previous);
                        self.cursor = before;
                    } else {
                        self.cursor = previous;
                    }
                } else {
                    self.cursor = previous;
                }
            }
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.text.insert(cursor, ch);
                self.cursor = cursor + ch.len_utf8();
            }
            _ => self.cursor = cursor,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayRoute {
    MessageOnly,
    AddTaskTitle,
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
    SwitchWorkspace,
    ConflictField,
    ConflictConfirm,
    ConflictManual,
    ConfigInit,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerState {
    pub(crate) route: OverlayRoute,
    pub(crate) title: String,
    pub(crate) filter: LineEdit,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
    pub(crate) multi: bool,
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
pub(crate) enum OverlayView {
    Help { scroll: u16 },
    Detail { scroll: u16 },
    DetailHelp { scroll: u16 },
    Search { input: String, cursor: usize },
    Command { input: String, cursor: usize },
    TextInput(TextInputView),
    MultilineInput(MultilineInputView),
    Picker(PickerView),
    Confirm(ConfirmView),
    TextPanel(TextPanelView),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelView {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextInputView {
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultilineInputView {
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerView {
    pub(crate) title: String,
    pub(crate) filter: String,
    pub(crate) filter_cursor: usize,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
    pub(crate) multi: bool,
    pub(crate) visible_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmView {
    pub(crate) title: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlaySubmit {
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
            Self::Text { title, .. } => format!("submitted {title}"),
            Self::Multiline { title, .. } => format!("submitted {title}"),
            Self::Picker { title, .. } => format!("selected {title}"),
            Self::Confirm { title, .. } => format!("confirmed {title}"),
        }
    }
}

impl OverlayState {
    pub(crate) fn captures_input(&self) -> bool {
        true
    }
}

impl OverlayView {
    pub(crate) fn captures_input(&self) -> bool {
        true
    }
}

impl From<&OverlayState> for OverlayView {
    fn from(state: &OverlayState) -> Self {
        match state {
            OverlayState::Help { scroll } => Self::Help { scroll: *scroll },
            OverlayState::Detail { scroll } => Self::Detail { scroll: *scroll },
            OverlayState::DetailHelp { scroll } => Self::DetailHelp { scroll: *scroll },
            OverlayState::Search { input } => Self::Search {
                input: input.text.clone(),
                cursor: input.cursor,
            },
            OverlayState::Command { input } => Self::Command {
                input: input.text.clone(),
                cursor: input.cursor,
            },
            OverlayState::TextInput(state) => Self::TextInput(TextInputView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                input: state.input.text.clone(),
                cursor: state.input.cursor,
            }),
            OverlayState::MultilineInput(state) => Self::MultilineInput(MultilineInputView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                lines: state.lines.clone(),
                row: state.row,
                column: state.column,
            }),
            OverlayState::Picker(state) => Self::Picker(PickerView {
                title: state.title.clone(),
                filter: state.filter.text.clone(),
                filter_cursor: state.filter.cursor,
                items: state.items.clone(),
                selected: state.selected,
                multi: state.multi,
                visible_indices: visible_picker_indices(state),
            }),
            OverlayState::Confirm(state) => Self::Confirm(ConfirmView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
            }),
            OverlayState::TextPanel(state) => Self::TextPanel(TextPanelView {
                title: state.title.clone(),
                lines: state.lines.clone(),
                scroll: state.scroll,
            }),
        }
    }
}

pub(crate) fn visible_picker_indices(state: &PickerState) -> Vec<usize> {
    let filter = state.filter.as_str().trim().to_ascii_lowercase();
    let dashless_filter = filter.replace('-', "");
    state
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| picker_item_matches(item, &filter, &dashless_filter))
        .map(|(index, _)| index)
        .collect()
}

fn picker_item_matches(item: &PickerItem, filter: &str, dashless_filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let label = item.label.to_ascii_lowercase();
    label.contains(filter) || label.replace('-', "").contains(dashless_filter)
}

pub(crate) fn normalize_picker_selection(state: &mut PickerState) {
    let visible = visible_picker_indices(state);
    state.selected = visible
        .iter()
        .copied()
        .find(|index| *index == state.selected)
        .or_else(|| visible.first().copied())
        .unwrap_or(0);
}

pub(crate) fn handle_generic_overlay_key(
    key: KeyEvent,
    overlay: OverlayState,
    help_scroll_cap: u16,
) -> OverlayOutcome {
    match overlay {
        OverlayState::TextInput(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Enter => OverlayOutcome::Submitted(OverlaySubmit::Text {
                route: state.route,
                title: state.title.clone(),
                value: state.input.text.clone(),
            }),
            _ => {
                state.input.handle_key(key);
                OverlayOutcome::None(OverlayState::TextInput(state))
            }
        },
        OverlayState::MultilineInput(mut state) => {
            if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let value = state.lines.join("\n");
                return OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                    route: state.route,
                    title: state.title.clone(),
                    value,
                });
            }
            match key.code {
                KeyCode::Esc => OverlayOutcome::Cancelled,
                _ => {
                    edit_multiline_input(&mut state, key);
                    OverlayOutcome::None(OverlayState::MultilineInput(state))
                }
            }
        }
        OverlayState::Picker(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Enter => {
                let values = if state.multi {
                    state
                        .items
                        .iter()
                        .filter(|item| item.selected)
                        .map(|item| item.value.clone())
                        .collect()
                } else {
                    visible_picker_indices(&state)
                        .iter()
                        .find(|index| **index == state.selected)
                        .map(|index| vec![state.items[*index].value.clone()])
                        .unwrap_or_default()
                };
                OverlayOutcome::Submitted(OverlaySubmit::Picker {
                    route: state.route,
                    title: state.title.clone(),
                    values,
                })
            }
            KeyCode::Down => {
                move_picker_selection(&mut state, 1);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
                move_picker_selection(&mut state, 1);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Up => {
                move_picker_selection(&mut state, -1);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                move_picker_selection(&mut state, -1);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Char(' ') if state.multi => {
                if let Some(index) = visible_picker_indices(&state)
                    .iter()
                    .find(|item| **item == state.selected)
                    .copied()
                {
                    state.items[index].selected = !state.items[index].selected;
                }
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            _ => {
                state.filter.handle_key(key);
                normalize_picker_selection(&mut state);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
        },
        OverlayState::Confirm(state) => match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => OverlayOutcome::Cancelled,
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                    route: state.route,
                    title: state.title.clone(),
                })
            }
            _ => OverlayOutcome::None(OverlayState::Confirm(state)),
        },
        OverlayState::TextPanel(mut state) => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                state.scroll = state.scroll.saturating_add(1);
                OverlayOutcome::None(OverlayState::TextPanel(state))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                state.scroll = state.scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::TextPanel(state))
            }
            _ => OverlayOutcome::None(OverlayState::TextPanel(state)),
        },
        OverlayState::Help { mut scroll } => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1).min(help_scroll_cap);
                OverlayOutcome::None(OverlayState::Help { scroll })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::Help { scroll })
            }
            _ => OverlayOutcome::None(OverlayState::Help { scroll }),
        },
        OverlayState::DetailHelp { mut scroll } => match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1).min(help_scroll_cap);
                OverlayOutcome::None(OverlayState::DetailHelp { scroll })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::DetailHelp { scroll })
            }
            _ => OverlayOutcome::None(OverlayState::DetailHelp { scroll }),
        },
        OverlayState::Detail { mut scroll } => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1);
                OverlayOutcome::None(OverlayState::Detail { scroll })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::Detail { scroll })
            }
            _ => OverlayOutcome::None(OverlayState::Detail { scroll }),
        },
        other => OverlayOutcome::None(other),
    }
}

fn edit_multiline_input(state: &mut MultilineInputState, key: KeyEvent) {
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }
    let row = state.row.min(state.lines.len() - 1);
    let column = char_boundary_at_or_before(&state.lines[row], state.column);
    state.row = row;
    state.column = column;

    match key.code {
        KeyCode::Up if row > 0 => {
            state.row = row - 1;
            state.column = char_boundary_at_or_before(&state.lines[state.row], state.column);
        }
        KeyCode::Down if row + 1 < state.lines.len() => {
            state.row = row + 1;
            state.column = char_boundary_at_or_before(&state.lines[state.row], state.column);
        }
        KeyCode::Left if column > 0 => {
            state.column = previous_char_boundary(&state.lines[row], column);
        }
        KeyCode::Right if column < state.lines[row].len() => {
            state.column = next_char_boundary(&state.lines[row], column);
        }
        KeyCode::Home => state.column = 0,
        KeyCode::End => state.column = state.lines[row].len(),
        KeyCode::Enter => {
            let rest = state.lines[row].split_off(column);
            state.lines.insert(row + 1, rest);
            state.row = row + 1;
            state.column = 0;
        }
        KeyCode::Backspace if column > 0 => {
            let previous = previous_char_boundary(&state.lines[row], column);
            state.lines[row].drain(previous..column);
            state.column = previous;
        }
        KeyCode::Backspace if row > 0 => {
            let line = state.lines.remove(row);
            state.row = row - 1;
            state.column = state.lines[state.row].len();
            state.lines[state.row].push_str(&line);
        }
        KeyCode::Delete if column < state.lines[row].len() => {
            let next = next_char_boundary(&state.lines[row], column);
            state.lines[row].drain(column..next);
        }
        KeyCode::Char(ch) => {
            state.lines[row].insert(column, ch);
            state.column = column + ch.len_utf8();
        }
        _ => state.column = column,
    }
}

fn char_boundary_at_or_before(input: &str, index: usize) -> usize {
    let mut index = index.min(input.len());
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn previous_char_boundary(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index).saturating_sub(1);
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_char_boundary(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index)
        .saturating_add(1)
        .min(input.len());
    while !input.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn previous_word_start(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index);
    while index > 0 {
        let previous = previous_char_boundary(input, index);
        if !input[previous..index].chars().all(char::is_whitespace) {
            break;
        }
        index = previous;
    }
    while index > 0 {
        let previous = previous_char_boundary(input, index);
        if input[previous..index].chars().all(char::is_whitespace) {
            break;
        }
        index = previous;
    }
    index
}

fn next_char_is_whitespace(input: &str, index: usize) -> bool {
    input[index..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
}

fn move_picker_selection(state: &mut PickerState, delta: isize) {
    let visible = visible_picker_indices(state);
    if visible.is_empty() {
        return;
    }
    let current = visible
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    let next = current as isize + delta;
    let next = if next < 0 {
        visible.len() - 1
    } else if next >= visible.len() as isize {
        0
    } else {
        next as usize
    };
    state.selected = visible[next];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn line_edit(input: &str, cursor: usize) -> LineEdit {
        LineEdit {
            text: input.to_string(),
            cursor,
        }
    }

    fn handle(key: KeyEvent, overlay: OverlayState) -> OverlayOutcome {
        handle_generic_overlay_key(key, overlay, 100)
    }

    fn handle_with_help_scroll_cap(
        key: KeyEvent,
        overlay: OverlayState,
        help_scroll_cap: u16,
    ) -> OverlayOutcome {
        handle_generic_overlay_key(key, overlay, help_scroll_cap)
    }

    #[test]
    fn text_input_edits_at_cursor() {
        let mut state = line_edit("ab", 1);
        state.handle_key(key(KeyCode::Char('x')));
        assert_eq!(state.text, "axb");
        assert_eq!(state.cursor, 2);
        state.handle_key(key(KeyCode::Backspace));
        assert_eq!(state.text, "ab");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn text_input_supports_emacs_navigation() {
        let mut state = line_edit("abc", 1);
        state.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(state.cursor, 0);
        state.handle_key(ctrl(KeyCode::Char('e')));
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('b')));
        assert_eq!(state.cursor, 2);
        state.handle_key(ctrl(KeyCode::Char('f')));
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn text_input_supports_emacs_deletion() {
        let mut state = line_edit("one two three", 7);
        state.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(state.text, "one three");
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('k')));
        assert_eq!(state.text, "one");
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('u')));
        assert_eq!(state.text, "");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn text_input_ignores_control_chars_that_are_not_editing_keys() {
        let mut state = line_edit("ab", 1);
        state.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(state.text, "ab");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn multiline_input_splits_and_merges_lines() {
        let mut state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["ab".to_string()],
            row: 0,
            column: 1,
        };
        edit_multiline_input(&mut state, key(KeyCode::Enter));
        assert_eq!(state.lines, vec!["a".to_string(), "b".to_string()]);
        state.row = 1;
        state.column = 0;
        edit_multiline_input(&mut state, key(KeyCode::Backspace));
        assert_eq!(state.lines, vec!["ab".to_string()]);
    }

    #[test]
    fn multiline_ctrl_s_submits() {
        let state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Notes".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line".to_string()],
            row: 0,
            column: 4,
        };
        let outcome = handle(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(state),
        );
        assert!(matches!(
            outcome,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline { .. })
        ));
    }

    #[test]
    fn picker_filter_and_selection_normalize() {
        let mut state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::blank(),
            items: vec![
                PickerItem {
                    label: "Alpha".to_string(),
                    value: "a".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Beta".to_string(),
                    value: "b".to_string(),
                    selected: false,
                },
            ],
            selected: 1,
            multi: false,
        };
        state.filter = LineEdit::new("alp".to_string());
        normalize_picker_selection(&mut state);
        assert_eq!(state.selected, 0);
        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_filter_ignores_dashes_in_labels() {
        let state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Go: project".to_string(),
            filter: LineEdit::new("gitsur".to_string()),
            items: vec![PickerItem {
                label: "GS git-surgeon".to_string(),
                value: "git-surgeon".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
        };

        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_filter_preserves_dash_matching() {
        let state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::new("git-sur".to_string()),
            items: vec![PickerItem {
                label: "GS git-surgeon".to_string(),
                value: "git-surgeon".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
        };

        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_types_j_and_k_into_filter() {
        let state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::blank(),
            items: vec![PickerItem {
                label: "jklabs".to_string(),
                value: "jklabs".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
        };
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Char('j')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Char('k')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.filter.as_str(), "jk");
    }

    #[test]
    fn picker_moves_with_arrow_keys() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Down), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(key(KeyCode::Up), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn picker_moves_with_ctrl_n_and_ctrl_p() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(ctrl(KeyCode::Char('n')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle(ctrl(KeyCode::Char('p')), OverlayState::Picker(state))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
    }

    fn picker_navigation_state() -> PickerState {
        PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::blank(),
            items: vec![
                PickerItem {
                    label: "Alpha".to_string(),
                    value: "a".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Beta".to_string(),
                    value: "b".to_string(),
                    selected: false,
                },
            ],
            selected: 0,
            multi: false,
        }
    }

    #[test]
    fn text_panel_closes_on_enter_and_esc() {
        let state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: vec!["field=title".to_string()],
            scroll: 0,
        };
        assert!(matches!(
            handle(key(KeyCode::Enter), OverlayState::TextPanel(state.clone())),
            OverlayOutcome::Cancelled
        ));
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::TextPanel(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn text_panel_scrolls_with_navigation_keys() {
        let state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: vec!["one".to_string(), "two".to_string()],
            scroll: 0,
        };
        let OverlayOutcome::None(OverlayState::TextPanel(state)) =
            handle(key(KeyCode::Down), OverlayState::TextPanel(state))
        else {
            panic!("expected scrolled text panel");
        };
        assert_eq!(state.scroll, 1);
        let OverlayOutcome::None(OverlayState::TextPanel(state)) =
            handle(key(KeyCode::Up), OverlayState::TextPanel(state))
        else {
            panic!("expected scrolled text panel");
        };
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn detail_scrolls_with_line_navigation_keys() {
        let OverlayOutcome::None(OverlayState::Detail { scroll }) =
            handle(key(KeyCode::Char('j')), OverlayState::Detail { scroll: 0 })
        else {
            panic!("expected scrolled detail");
        };
        assert_eq!(scroll, 1);
        let OverlayOutcome::None(OverlayState::Detail { scroll }) =
            handle(key(KeyCode::Char('k')), OverlayState::Detail { scroll })
        else {
            panic!("expected scrolled detail");
        };
        assert_eq!(scroll, 0);
    }

    #[test]
    fn esc_cancels_all_generic_overlay_variants() {
        let overlays = vec![
            OverlayState::TextInput(TextInputState::new(
                OverlayRoute::MessageOnly,
                "Title",
                "Prompt",
                "value".to_string(),
            )),
            OverlayState::MultilineInput(MultilineInputState {
                route: OverlayRoute::MessageOnly,
                title: "Body".to_string(),
                prompt: "Prompt".to_string(),
                lines: vec!["value".to_string()],
                row: 0,
                column: 5,
            }),
            OverlayState::Picker(PickerState {
                route: OverlayRoute::MessageOnly,
                title: "Pick".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
            }),
            OverlayState::Confirm(ConfirmState {
                route: OverlayRoute::MessageOnly,
                title: "Confirm".to_string(),
                prompt: "Continue?".to_string(),
            }),
            OverlayState::TextPanel(TextPanelState {
                title: "Panel".to_string(),
                lines: vec!["line".to_string()],
                scroll: 0,
            }),
        ];

        for overlay in overlays {
            assert!(matches!(
                handle(key(KeyCode::Esc), overlay),
                OverlayOutcome::Cancelled
            ));
        }
    }

    #[test]
    fn help_scroll_stops_at_cap() {
        let OverlayOutcome::None(OverlayState::Help { scroll }) =
            handle_with_help_scroll_cap(key(KeyCode::Down), OverlayState::Help { scroll: 2 }, 2)
        else {
            panic!("expected help overlay state");
        };
        assert_eq!(scroll, 2);
    }

    #[test]
    fn confirm_yes_and_no() {
        let state = ConfirmState {
            route: OverlayRoute::MessageOnly,
            title: "Delete".to_string(),
            prompt: "Sure?".to_string(),
        };
        assert!(matches!(
            handle(
                key(KeyCode::Char('y')),
                OverlayState::Confirm(state.clone())
            ),
            OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                route: OverlayRoute::MessageOnly,
                title,
                ..
            }) if title == "Delete"
        ));
        assert!(matches!(
            handle(key(KeyCode::Char('n')), OverlayState::Confirm(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn generic_submit_variants_propagate_route() {
        let text = handle(
            key(KeyCode::Enter),
            OverlayState::TextInput(TextInputState::new(
                OverlayRoute::AddProject,
                "Add project",
                "name:",
                "app".to_string(),
            )),
        );
        assert!(matches!(
            text,
            OverlayOutcome::Submitted(OverlaySubmit::Text {
                route: OverlayRoute::AddProject,
                ..
            })
        ));

        let multiline = handle(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(MultilineInputState {
                route: OverlayRoute::AddNote,
                title: "Add note".to_string(),
                prompt: "body:".to_string(),
                lines: vec!["note".to_string()],
                row: 0,
                column: 4,
            }),
        );
        assert!(matches!(
            multiline,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                route: OverlayRoute::AddNote,
                ..
            })
        ));

        let picker = handle(
            key(KeyCode::Enter),
            OverlayState::Picker(PickerState {
                route: OverlayRoute::EditStatus,
                title: "Edit task: status".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "Todo".to_string(),
                    value: "todo".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
            }),
        );
        assert!(matches!(
            picker,
            OverlayOutcome::Submitted(OverlaySubmit::Picker {
                route: OverlayRoute::EditStatus,
                ..
            })
        ));
    }
}

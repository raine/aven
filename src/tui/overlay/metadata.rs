use aven_core::metadata::MetadataField;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};

use super::{LineEdit, OverlayOutcome, OverlayState, OverlaySubmit, TextBuffer};
use crate::tui::task_selection::TaskSelection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataTarget {
    pub(crate) workspace_id: crate::ids::WorkspaceId,
    pub(crate) selection: TaskSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataEntry {
    pub(crate) field: MetadataField,
    pub(crate) value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataFocus {
    Input,
    Save,
    ExternalEditor,
    Remove,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataEditor {
    pub(crate) input: TextBuffer,
    pub(crate) focus: MetadataFocus,
    pub(crate) discard: bool,
}

pub(crate) fn metadata_display(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

impl MetadataEditor {
    pub(crate) fn is_multiline(&self) -> bool {
        self.input.lines.len() > 1 || self.input.lines.iter().any(|line| line.contains('\r'))
    }

    pub(crate) fn can_save(&self) -> bool {
        self.input.lines.iter().any(|line| !line.trim().is_empty())
    }

    pub(crate) fn viewport(&self, area: Rect) -> (usize, usize, usize) {
        use unicode_width::UnicodeWidthStr;
        let row = self.input.row.min(self.input.lines.len().saturating_sub(1));
        let top = row.saturating_sub(area.height.saturating_sub(1) as usize);
        let column = self
            .input
            .lines
            .get(row)
            .map(|line| {
                let boundary =
                    crate::tui::text::char_boundary_at_or_before(line, self.input.column);
                metadata_display(&line[..boundary]).width()
            })
            .unwrap_or(0);
        (
            top,
            column.saturating_sub(area.width.saturating_sub(1) as usize),
            column,
        )
    }

    fn click(&mut self, area: Rect, column: u16, row: u16) {
        let (top, left, _) = self.viewport(area);
        self.input.row = (top + row.saturating_sub(area.y) as usize)
            .min(self.input.lines.len().saturating_sub(1));
        let desired = left + column.saturating_sub(area.x) as usize;
        let line = &self.input.lines[self.input.row];
        let mut cells = 0;
        self.input.column = line.len();
        for (index, c) in line.char_indices() {
            let width =
                unicode_width::UnicodeWidthChar::width(if c.is_control() { '�' } else { c })
                    .unwrap_or(0);
            if cells + width > desired {
                self.input.column = index;
                break;
            }
            cells += width;
        }
        self.focus = MetadataFocus::Input;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataState {
    pub(crate) target: MetadataTarget,
    pub(crate) entries: Vec<MetadataEntry>,
    pub(crate) filter: LineEdit,
    pub(crate) selected: usize,
    pub(crate) editor: Option<MetadataEditor>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetadataView<'a> {
    pub(crate) entries: &'a [MetadataEntry],
    pub(crate) filter: &'a LineEdit,
    pub(crate) selected: usize,
    pub(crate) editor: Option<&'a MetadataEditor>,
    pub(crate) error: Option<&'a str>,
}

impl MetadataState {
    pub(crate) fn view(&self) -> MetadataView<'_> {
        MetadataView {
            entries: &self.entries,
            filter: &self.filter,
            selected: self.selected,
            editor: self.editor.as_ref(),
            error: self.error.as_deref(),
        }
    }

    fn open(&mut self) {
        let Some(entry) = self.entries.get(self.selected) else {
            return;
        };
        self.editor = Some(MetadataEditor {
            input: TextBuffer::from_value(entry.value.clone().unwrap_or_default()),
            focus: MetadataFocus::Input,
            discard: false,
        });
        if let Some(editor) = &mut self.editor
            && editor.is_multiline()
        {
            editor.focus = MetadataFocus::ExternalEditor;
        }
        self.error = None;
    }

    pub(crate) fn paste(&mut self, text: &str) {
        if let Some(editor) = &mut self.editor {
            if !editor.discard && editor.focus == MetadataFocus::Input && !editor.is_multiline() {
                editor.input.insert_exact(text);
                if editor.is_multiline() {
                    editor.focus = MetadataFocus::ExternalEditor;
                }
                self.error = None;
            }
        } else {
            self.filter.insert_paste(text);
            self.normalize_selection();
        }
    }

    pub(crate) fn normalize_selection(&mut self) {
        let visible = self.view().visible();
        if !visible.contains(&self.selected) {
            self.selected = visible.first().copied().unwrap_or(0);
        }
    }

    fn move_selection(&mut self, reverse: bool) {
        let visible = self.view().visible();
        if visible.is_empty() {
            return;
        }
        let index = visible
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        self.selected = visible[if reverse {
            (index + visible.len() - 1) % visible.len()
        } else {
            (index + 1) % visible.len()
        }];
    }

    fn cancel_edit(&mut self) {
        if let Some(editor) = &mut self.editor {
            if editor.input.is_dirty() {
                editor.discard = true;
            } else {
                self.editor = None;
            }
        }
        self.error = None;
    }
}

impl MetadataView<'_> {
    pub(crate) fn visible(&self) -> Vec<usize> {
        let filter = self.filter.text.to_ascii_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| entry.field.key.contains(&filter).then_some(i))
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MetadataAction {
    pub(crate) focus: MetadataFocus,
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) visible: bool,
    pub(crate) enabled: bool,
    pub(crate) area: Rect,
}

impl MetadataView<'_> {
    fn actions(&self, width: u16) -> [MetadataAction; 4] {
        let editor = self.editor.expect("metadata editor actions");
        let assigned = self.entries[self.selected].value.is_some();
        [
            (
                MetadataFocus::Save,
                if matches!(editor.focus, MetadataFocus::Input | MetadataFocus::Save) {
                    "Enter"
                } else {
                    "^S"
                },
                "save",
                true,
                editor.can_save(),
            ),
            (
                MetadataFocus::ExternalEditor,
                if width >= 60 {
                    "Ctrl+X Ctrl+E"
                } else {
                    "^X ^E"
                },
                "editor",
                true,
                true,
            ),
            (
                MetadataFocus::Remove,
                if editor.focus == MetadataFocus::Remove {
                    "Enter"
                } else {
                    ""
                },
                if width < 30 { "remove" } else { "remove field" },
                assigned,
                assigned,
            ),
            (MetadataFocus::Cancel, "Esc", "cancel", true, true),
        ]
        .map(|(focus, key, label, visible, enabled)| MetadataAction {
            focus,
            key,
            label,
            visible,
            enabled,
            area: Rect::default(),
        })
    }
}

pub(crate) struct MetadataLayout {
    pub(crate) area: Rect,
    pub(crate) header: Rect,
    pub(crate) body: Rect,
    pub(crate) error: Rect,
    pub(crate) actions: Vec<MetadataAction>,
    pub(crate) browser_hints: Rect,
    pub(crate) done: Rect,
}

fn metadata_footer(view: &MetadataView<'_>, width: u16) -> (Vec<MetadataAction>, u16) {
    let mut x = 0;
    let mut y = 0;
    let actions = view
        .actions(width)
        .into_iter()
        .map(|mut action| {
            if action.visible {
                let length = (action.key.len()
                    + usize::from(!action.key.is_empty())
                    + action.label.len()) as u16;
                let length = length.min(width);
                if x > 0 && x + length > width {
                    x = 0;
                    y += 1;
                }
                action.area = Rect::new(x, y, length, 1);
                x += length + 2;
            }
            action
        })
        .collect();
    (actions, y + 1)
}

pub(crate) fn metadata_layout(view: &MetadataView<'_>, size: Size) -> MetadataLayout {
    let bounds = Rect::new(0, 0, size.width, size.height);
    let width = super::layout::dialog_inner_area(super::dialog_area(bounds, 72, size.height)).width;
    let (footer, action_rows) = if view.editor.is_some() {
        metadata_footer(view, width)
    } else {
        (Vec::new(), 1)
    };
    let hint_rows = if view.error.is_some() {
        2
    } else if view.editor.is_some_and(|editor| editor.is_multiline()) {
        1
    } else {
        0
    };
    let height = if let Some(editor) = view.editor {
        editor.input.lines.len().clamp(1, 3) as u16 + 5 + hint_rows + action_rows - 1
    } else {
        view.visible().len().clamp(3, 10) as u16 + 6
    };
    let area = super::dialog_area(Rect::new(0, 0, size.width, size.height), 72, height);
    let inner = super::layout::dialog_inner_area(area);
    let rows = inner.height;
    let header = Rect::new(inner.x, inner.y, inner.width, rows.min(1));
    let action_y = inner.bottom().saturating_sub(action_rows).max(inner.y);
    let error_y = action_y
        .saturating_sub(
            (if view.editor.is_some() { hint_rows } else { 1 })
                .min(rows.saturating_sub(action_rows + 2)),
        )
        .max(inner.y.saturating_add(header.height))
        .min(action_y);
    let gap = u16::from(view.editor.is_none() && rows >= 6);
    let body_y = inner.y.saturating_add(header.height + gap).min(action_y);
    let body_bottom = error_y
        .saturating_sub(u16::from(view.editor.is_some() && error_y > body_y + 1))
        .max(body_y);
    let body = Rect::new(
        inner.x,
        body_y,
        inner.width,
        body_bottom.saturating_sub(body_y),
    );
    let error = Rect::new(
        inner.x,
        error_y,
        inner.width,
        action_y.saturating_sub(error_y),
    );
    let actions = footer
        .into_iter()
        .map(|mut action| {
            if action.visible {
                action.area.x += inner.x;
                action.area.y += action_y;
            }
            action
        })
        .collect();
    let done_start = inner.width as usize * 3 / 4;
    let browser_hints = Rect::new(inner.x, action_y, done_start as u16, u16::from(rows > 0));
    let done = Rect::new(
        inner.x + done_start as u16,
        action_y,
        inner.width - done_start as u16,
        u16::from(rows > 0),
    );
    MetadataLayout {
        area,
        header,
        body,
        error,
        actions,
        browser_hints,
        done,
    }
}

pub(crate) fn visible_start(visible: &[usize], selected: usize, rows: usize) -> usize {
    visible
        .iter()
        .position(|i| *i == selected)
        .unwrap_or(0)
        .saturating_sub(rows.saturating_sub(1))
}

fn activate(mut state: Box<MetadataState>, focus: MetadataFocus) -> OverlayOutcome {
    let focus = if focus == MetadataFocus::Input {
        MetadataFocus::Save
    } else {
        focus
    };
    if state.editor.as_ref().is_some_and(|editor| !editor.discard)
        && state
            .view()
            .actions(0)
            .iter()
            .any(|action| action.focus == focus && action.visible && action.enabled)
    {
        match focus {
            MetadataFocus::Save | MetadataFocus::Remove => {
                return OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                    state,
                    remove: focus == MetadataFocus::Remove,
                });
            }
            MetadataFocus::ExternalEditor => {
                return OverlayOutcome::Submitted(OverlaySubmit::MetadataExternalEditor(state));
            }
            MetadataFocus::Cancel => state.cancel_edit(),
            MetadataFocus::Input => unreachable!("input activates save"),
        }
    }
    OverlayOutcome::None(OverlayState::Metadata(state))
}

pub(crate) fn handle_key(mut state: Box<MetadataState>, key: KeyEvent) -> OverlayOutcome {
    if let Some(editor) = &mut state.editor {
        if editor.discard {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    state.editor = None;
                    state.error = None;
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => editor.discard = false,
                _ => {}
            }
        } else if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('s') | KeyCode::Enter)
        {
            return activate(state, MetadataFocus::Save);
        } else {
            match key.code {
                KeyCode::Esc => state.cancel_edit(),
                KeyCode::Tab | KeyCode::BackTab => {
                    let focus = editor.focus;
                    let input = (!editor.is_multiline()).then_some(MetadataFocus::Input);
                    let choices: Vec<_> = input
                        .into_iter()
                        .chain(
                            state
                                .view()
                                .actions(0)
                                .into_iter()
                                .filter(|action| action.visible && action.enabled)
                                .map(|action| action.focus),
                        )
                        .collect();
                    let i = choices
                        .iter()
                        .position(|choice| *choice == focus)
                        .unwrap_or(0);
                    state.editor.as_mut().unwrap().focus = choices[if key.code == KeyCode::BackTab {
                        (i + choices.len() - 1) % choices.len()
                    } else {
                        (i + 1) % choices.len()
                    }];
                }
                KeyCode::Enter => {
                    let focus = editor.focus;
                    return activate(state, focus);
                }
                _ if editor.focus == MetadataFocus::Input && !editor.is_multiline() => {
                    super::text_buffer::edit_text_buffer(&mut editor.input, key);
                    state.error = None;
                }
                _ => {}
            }
        }
    } else {
        match key.code {
            KeyCode::Esc => return OverlayOutcome::Cancelled,
            KeyCode::Up | KeyCode::BackTab => state.move_selection(true),
            KeyCode::Down | KeyCode::Tab => state.move_selection(false),
            KeyCode::Enter if !state.view().visible().is_empty() => state.open(),
            _ => {
                state.filter.handle_key(key);
                state.normalize_selection();
            }
        }
    }
    OverlayOutcome::None(OverlayState::Metadata(state))
}

pub(crate) fn handle_mouse(
    mut state: Box<MetadataState>,
    mouse: MouseEvent,
    size: Size,
) -> OverlayOutcome {
    let layout = metadata_layout(&state.view(), size);
    let point = (mouse.column, mouse.row).into();
    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
        if let Some(editor) = &mut state.editor {
            if editor.discard {
                return OverlayOutcome::None(OverlayState::Metadata(state));
            } else if layout.body.contains(point) && !editor.is_multiline() {
                editor.click(layout.body, mouse.column, mouse.row);
            } else if let Some(action) = layout
                .actions
                .iter()
                .find(|action| action.visible && action.enabled && action.area.contains(point))
            {
                return activate(state, action.focus);
            }
        } else if layout.body.contains(point) {
            let view = state.view();
            let visible = view.visible();
            let index = visible_start(&visible, view.selected, layout.body.height as usize)
                + (mouse.row - layout.body.y) as usize;
            if let Some(selected) = visible.get(index) {
                state.selected = *selected;
                state.open();
            }
        } else if layout.done.contains(point) {
            return OverlayOutcome::Cancelled;
        }
    } else if state.editor.is_none() && layout.body.contains(point) {
        match mouse.kind {
            MouseEventKind::ScrollDown => state.move_selection(false),
            MouseEventKind::ScrollUp => state.move_selection(true),
            _ => {}
        }
    }
    OverlayOutcome::None(OverlayState::Metadata(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_geometry_keeps_actions_and_input_reachable() {
        for (width, height) in [(120, 40), (80, 24), (40, 12), (30, 8)] {
            let filter = LineEdit::blank();
            let view = MetadataView {
                entries: &[],
                filter: &filter,
                selected: 0,
                editor: None,
                error: None,
            };
            let layout = metadata_layout(&view, Size::new(width, height));
            assert!(layout.body.height > 0);
            assert!(layout.body.width > 0);
            assert!(layout.body.bottom() <= layout.error.y);
            for action in [layout.browser_hints, layout.done] {
                assert!(action.width > 0);
                assert!(action.bottom() < height);
                assert!(action.right() < width);
            }
        }
    }

    #[test]
    fn editor_mouse_uses_unicode_and_control_safe_viewport() {
        let mut editor = MetadataEditor {
            input: TextBuffer::from_value("é\t中\nlast".to_string()),
            focus: MetadataFocus::Save,
            discard: false,
        };
        let area = Rect::new(3, 2, 8, 3);
        editor.click(area, 5, 2);
        assert_eq!(editor.input.row, 0);
        assert_eq!(editor.input.column, "é\t".len());
        assert_eq!(editor.focus, MetadataFocus::Input);
        assert_eq!(metadata_display("é\t中"), "é�中");
        editor.input.insert_exact("\r\n");
        assert_eq!(editor.input.lines.join("\n"), "é\t\r\n中\nlast");
    }

    #[test]
    fn filtering_keeps_field_identity_and_scroll_tracks_selected_row() {
        let entries = (0..30)
            .map(|i| MetadataEntry {
                field: MetadataField {
                    id: crate::ids::MetadataFieldId::new(),
                    workspace_id: crate::ids::WorkspaceId::new(),
                    key: format!("field_{i:02}"),
                    created_at: String::new(),
                    updated_at: String::new(),
                },
                value: if i == 0 { Some(String::new()) } else { None },
            })
            .collect::<Vec<_>>();
        let filter = LineEdit::new("FIELD_2".to_string());
        let view = MetadataView {
            entries: &entries,
            filter: &filter,
            selected: 29,
            editor: None,
            error: None,
        };
        assert_eq!(view.visible(), (20..30).collect::<Vec<_>>());
        assert_eq!(visible_start(&view.visible(), view.selected, 4), 6);
        assert_eq!(entries[0].value.as_deref(), Some(""));
        assert_eq!(entries[1].value, None);
    }

    fn state(values: &[Option<&str>]) -> Box<MetadataState> {
        let task = crate::tui::test_support::task_list_item_with_id("metadata", "target");
        Box::new(MetadataState {
            target: MetadataTarget {
                workspace_id: crate::ids::WorkspaceId::new(),
                selection: TaskSelection::resolve_single(&[task], Some(0)).unwrap(),
            },
            entries: values
                .iter()
                .enumerate()
                .map(|(i, value)| MetadataEntry {
                    field: MetadataField {
                        id: crate::ids::MetadataFieldId::new(),
                        workspace_id: crate::ids::WorkspaceId::new(),
                        key: format!("field_{i:02}"),
                        created_at: String::new(),
                        updated_at: String::new(),
                    },
                    value: value.map(str::to_string),
                })
                .collect(),
            filter: LineEdit::blank(),
            selected: 0,
            editor: None,
            error: None,
        })
    }

    fn retained(outcome: OverlayOutcome) -> Box<MetadataState> {
        let OverlayOutcome::None(OverlayState::Metadata(state)) = outcome else {
            panic!("expected retained metadata")
        };
        state
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, area: Rect) -> MouseEvent {
        MouseEvent {
            kind,
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_and_focus_use_narrow_rendered_geometry() {
        let state = state(&[Some("pending"), Some("")]);
        let size = Size::new(40, 12);
        let layout = metadata_layout(&state.view(), size);
        let context = super::super::OverlayMouseContext {
            add_task_only: false,
            detail_help_scroll_cap: 0,
        };
        let outcome = super::super::dispatch_overlay_mouse(
            OverlayState::Metadata(state),
            mouse(MouseEventKind::ScrollDown, layout.body),
            size,
            context,
        );
        let super::super::OverlayMouseOutcome::Retained(OverlayState::Metadata(state)) = outcome
        else {
            panic!()
        };
        assert_eq!(state.selected, 1);
        let outcome = super::super::dispatch_overlay_mouse(
            OverlayState::Metadata(state),
            mouse(MouseEventKind::Down(MouseButton::Left), layout.body),
            size,
            context,
        );
        let super::super::OverlayMouseOutcome::Retained(OverlayState::Metadata(state)) = outcome
        else {
            panic!()
        };
        let state = retained(handle_key(state, key(KeyCode::Tab)));
        assert_eq!(state.editor.as_ref().unwrap().focus, MetadataFocus::Save);
        let state = retained(handle_key(state, key(KeyCode::BackTab)));
        assert_eq!(state.editor.as_ref().unwrap().focus, MetadataFocus::Input);
    }

    #[test]
    fn actions_share_focus_activation_and_packed_hit_areas() {
        for assigned in [false, true] {
            for value in ["", " \t", "value", "line\r\nnext"] {
                for width in [100, 70, 40, 30] {
                    let mut state = state(&[assigned.then_some("original")]);
                    state.open();
                    state.editor.as_mut().unwrap().input = TextBuffer::from_value_with_baseline(
                        value.to_string(),
                        (if assigned { "original" } else { "" }).to_string(),
                    );
                    let multiline = state.editor.as_ref().unwrap().is_multiline();
                    let can_save = state.editor.as_ref().unwrap().can_save();
                    let mut expected = Vec::new();
                    if !multiline {
                        expected.push(MetadataFocus::Input);
                    }
                    if can_save {
                        expected.push(MetadataFocus::Save);
                    }
                    expected.push(MetadataFocus::ExternalEditor);
                    if assigned {
                        expected.push(MetadataFocus::Remove);
                    }
                    expected.push(MetadataFocus::Cancel);
                    for focus in [
                        MetadataFocus::Input,
                        MetadataFocus::Save,
                        MetadataFocus::ExternalEditor,
                        MetadataFocus::Remove,
                        MetadataFocus::Cancel,
                    ] {
                        state.editor.as_mut().unwrap().focus = focus;
                        let index = expected.iter().position(|item| *item == focus).unwrap_or(0);
                        for (code, next) in [
                            (KeyCode::Tab, (index + 1) % expected.len()),
                            (
                                KeyCode::BackTab,
                                (index + expected.len() - 1) % expected.len(),
                            ),
                        ] {
                            let result = retained(handle_key(state.clone(), key(code)));
                            assert_eq!(result.editor.as_ref().unwrap().focus, expected[next]);
                        }
                    }
                    let size = Size::new(width, 24);
                    let layout = metadata_layout(&state.view(), size);
                    for action in &layout.actions {
                        if !action.visible {
                            assert_eq!(action.area, Rect::default());
                            continue;
                        }
                        assert!(action.area.width > 0);
                        assert!(action.area.right() < width);
                        assert_eq!(
                            layout
                                .actions
                                .iter()
                                .filter(|other| other.visible
                                    && other.area.contains((action.area.x, action.area.y).into()))
                                .count(),
                            1
                        );
                        state.editor.as_mut().unwrap().focus = action.focus;
                        for outcome in [
                            handle_key(state.clone(), key(KeyCode::Enter)),
                            handle_mouse(
                                state.clone(),
                                mouse(MouseEventKind::Down(MouseButton::Left), action.area),
                                size,
                            ),
                        ] {
                            match (action.focus, action.enabled, outcome) {
                                (
                                    MetadataFocus::Save,
                                    true,
                                    OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                                        remove: false,
                                        ..
                                    }),
                                ) => {}
                                (
                                    MetadataFocus::Remove,
                                    true,
                                    OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                                        remove: true,
                                        ..
                                    }),
                                ) => {}
                                (
                                    MetadataFocus::ExternalEditor,
                                    true,
                                    OverlayOutcome::Submitted(
                                        OverlaySubmit::MetadataExternalEditor(_),
                                    ),
                                ) => {}
                                (
                                    MetadataFocus::Cancel,
                                    _,
                                    OverlayOutcome::None(OverlayState::Metadata(result)),
                                ) => {
                                    if value == if assigned { "original" } else { "" } {
                                        assert!(result.editor.is_none());
                                    } else {
                                        assert!(result.editor.as_ref().unwrap().discard);
                                    }
                                }
                                (
                                    MetadataFocus::Save,
                                    false,
                                    OverlayOutcome::None(OverlayState::Metadata(result)),
                                ) => assert_eq!(result, state),
                                _ => panic!("incorrect action outcome: {action:?}"),
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn filtered_scrolled_clicks_keep_entry_indices_and_empty_results_do_nothing() {
        let mut state = state(&vec![None; 30]);
        state.filter = LineEdit::new("FIELD_2".to_string());
        state.selected = 29;
        let size = Size::new(40, 12);
        let layout = metadata_layout(&state.view(), size);
        let visible = state.view().visible();
        let start = visible_start(&visible, 29, layout.body.height as usize);
        let result = retained(handle_mouse(
            state.clone(),
            mouse(MouseEventKind::Down(MouseButton::Left), layout.body),
            size,
        ));
        assert_eq!(result.selected, visible[start]);
        assert!(result.editor.is_some());
        assert_eq!(visible_start(&visible, 99, 4), 0);
        assert_eq!(visible_start(&visible, 29, 0), 9);
        state.filter = LineEdit::new("missing".to_string());
        state.normalize_selection();
        assert_eq!(state.selected, 0);
        assert_eq!(visible_start(&[], 0, 0), 0);
        let layout = metadata_layout(&state.view(), size);
        let result = retained(handle_mouse(
            state.clone(),
            mouse(MouseEventKind::Down(MouseButton::Left), layout.body),
            size,
        ));
        assert_eq!(result, state);
    }

    #[test]
    fn discard_blocks_keyboard_and_every_action_click() {
        let mut state = state(&[Some("original")]);
        state.open();
        state.paste("!");
        state.cancel_edit();
        let layout = metadata_layout(&state.view(), Size::new(40, 12));
        for event in [
            key(KeyCode::Enter),
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        ] {
            assert_eq!(retained(handle_key(state.clone(), event)), state);
        }
        for action in layout.actions {
            assert_eq!(
                retained(handle_mouse(
                    state.clone(),
                    mouse(MouseEventKind::Down(MouseButton::Left), action.area),
                    Size::new(40, 12)
                )),
                state
            );
        }
        for code in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('N')] {
            let result = retained(handle_key(state.clone(), key(code)));
            assert!(!result.editor.as_ref().unwrap().discard);
            assert_eq!(
                result.editor.as_ref().unwrap().input,
                state.editor.as_ref().unwrap().input
            );
        }
        for code in [KeyCode::Char('y'), KeyCode::Char('Y')] {
            assert!(
                retained(handle_key(state.clone(), key(code)))
                    .editor
                    .is_none()
            );
        }
    }
}

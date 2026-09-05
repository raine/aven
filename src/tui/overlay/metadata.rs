use aven_core::metadata::MetadataField;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};

use super::{
    LineEdit, MultilineInputState, MultilineIntent, OverlayOutcome, OverlayState, OverlaySubmit,
};
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
    pub(crate) input: MultilineInputState,
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
            input: MultilineInputState::from_value(
                MultilineIntent::CustomMetadata,
                "",
                "",
                entry.value.clone().unwrap_or_default(),
            ),
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
                editor.input.insert_paste(text);
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

pub(crate) struct MetadataLayout {
    pub(crate) area: Rect,
    pub(crate) header: Rect,
    pub(crate) body: Rect,
    pub(crate) error: Rect,
    pub(crate) actions: [Rect; 4],
}

pub(crate) fn metadata_footer_hints(
    view: &MetadataView<'_>,
    width: u16,
) -> [(&'static str, &'static str); 4] {
    let editor = view.editor.expect("metadata editor footer");
    [
        (
            if matches!(editor.focus, MetadataFocus::Input | MetadataFocus::Save) {
                "Enter"
            } else {
                "^S"
            },
            "save",
        ),
        (
            if width >= 60 {
                "Ctrl+X Ctrl+E"
            } else {
                "^X ^E"
            },
            "editor",
        ),
        (
            if editor.focus == MetadataFocus::Remove {
                "Enter"
            } else {
                ""
            },
            if view.entries[view.selected].value.is_none() {
                ""
            } else if width < 30 {
                "remove"
            } else {
                "remove field"
            },
        ),
        ("Esc", "cancel"),
    ]
}

fn metadata_footer_rects(view: &MetadataView<'_>, width: u16) -> ([Rect; 4], u16) {
    let mut x = 0;
    let mut y = 0;
    let actions = metadata_footer_hints(view, width).map(|(key, label)| {
        if label.is_empty() {
            return Rect::default();
        }
        let length = (key.len() + usize::from(!key.is_empty()) + label.len()) as u16;
        let length = length.min(width);
        if x > 0 && x + length > width {
            x = 0;
            y += 1;
        }
        let area = Rect::new(x, y, length, 1);
        x += length + 2;
        area
    });
    (actions, y + 1)
}

pub(crate) fn metadata_layout(view: &MetadataView<'_>, size: Size) -> MetadataLayout {
    let bounds = Rect::new(0, 0, size.width, size.height);
    let width = super::layout::dialog_inner_area(super::dialog_area(bounds, 72, size.height)).width;
    let (footer, action_rows) = if view.editor.is_some() {
        metadata_footer_rects(view, width)
    } else {
        ([Rect::default(); 4], 1)
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
    let actions = if view.editor.is_some() {
        footer.map(|area| Rect::new(inner.x + area.x, action_y + area.y, area.width, area.height))
    } else {
        std::array::from_fn(|i| {
            let start = inner.width as usize * i / 4;
            let end = inner.width as usize * (i + 1) / 4;
            Rect::new(
                inner.x + start as u16,
                action_y,
                (end - start) as u16,
                u16::from(rows > 0),
            )
        })
    };
    MetadataLayout {
        area,
        header,
        body,
        error,
        actions,
    }
}

pub(crate) fn visible_start(view: &MetadataView<'_>, rows: usize) -> usize {
    view.visible()
        .iter()
        .position(|i| *i == view.selected)
        .unwrap_or(0)
        .saturating_sub(rows.saturating_sub(1))
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
            if editor.can_save() {
                return OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                    state,
                    remove: false,
                });
            }
        } else {
            match key.code {
                KeyCode::Esc => state.cancel_edit(),
                KeyCode::Tab | KeyCode::BackTab => {
                    let mut choices = Vec::new();
                    if !editor.is_multiline() {
                        choices.push(MetadataFocus::Input);
                    }
                    if editor.can_save() {
                        choices.push(MetadataFocus::Save);
                    }
                    choices.push(MetadataFocus::ExternalEditor);
                    if state.entries[state.selected].value.is_some() {
                        choices.push(MetadataFocus::Remove);
                    }
                    choices.push(MetadataFocus::Cancel);
                    let i = choices
                        .iter()
                        .position(|focus| *focus == editor.focus)
                        .unwrap_or(0);
                    editor.focus = choices[if key.code == KeyCode::BackTab {
                        (i + choices.len() - 1) % choices.len()
                    } else {
                        (i + 1) % choices.len()
                    }];
                }
                KeyCode::Enter => match editor.focus {
                    MetadataFocus::Save | MetadataFocus::Input if editor.can_save() => {
                        return OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                            state,
                            remove: false,
                        });
                    }
                    MetadataFocus::Remove => {
                        return OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                            state,
                            remove: true,
                        });
                    }
                    MetadataFocus::Cancel => state.cancel_edit(),
                    MetadataFocus::ExternalEditor => {
                        return OverlayOutcome::Submitted(OverlaySubmit::MetadataExternalEditor(
                            state,
                        ));
                    }
                    MetadataFocus::Save | MetadataFocus::Input => {}
                },
                _ if editor.focus == MetadataFocus::Input && !editor.is_multiline() => {
                    super::multiline::edit_multiline_input(&mut editor.input, key);
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
            } else if layout.actions[0].contains(point) && editor.can_save() {
                return OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                    state,
                    remove: false,
                });
            } else if layout.actions[1].contains(point) {
                return OverlayOutcome::Submitted(OverlaySubmit::MetadataExternalEditor(state));
            } else if layout.actions[2].contains(point)
                && state.entries[state.selected].value.is_some()
            {
                return OverlayOutcome::Submitted(OverlaySubmit::MetadataSave {
                    state,
                    remove: true,
                });
            } else if layout.actions[3].contains(point) {
                state.cancel_edit();
            }
        } else if layout.body.contains(point) {
            let view = state.view();
            let index = visible_start(&view, layout.body.height as usize)
                + (mouse.row - layout.body.y) as usize;
            if let Some(selected) = view.visible().get(index) {
                state.selected = *selected;
                state.open();
            }
        } else if layout.actions[3].contains(point) {
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
            for action in layout.actions {
                assert!(action.width > 0);
                assert!(action.bottom() < height);
                assert!(action.right() < width);
            }
        }
    }

    #[test]
    fn editor_mouse_uses_unicode_and_control_safe_viewport() {
        let mut editor = MetadataEditor {
            input: MultilineInputState::from_value(
                MultilineIntent::CustomMetadata,
                "",
                "",
                "é\t中\nlast".to_string(),
            ),
            focus: MetadataFocus::Save,
            discard: false,
        };
        let area = Rect::new(3, 2, 8, 3);
        editor.click(area, 5, 2);
        assert_eq!(editor.input.row, 0);
        assert_eq!(editor.input.column, "é\t".len());
        assert_eq!(editor.focus, MetadataFocus::Input);
        assert_eq!(metadata_display("é\t中"), "é�中");
        editor.input.insert_paste("\r\n");
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
        assert_eq!(visible_start(&view, 4), 6);
        assert_eq!(entries[0].value.as_deref(), Some(""));
        assert_eq!(entries[1].value, None);
    }
}

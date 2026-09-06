use ratatui::layout::{Constraint, Flex, Layout, Rect, Size};
use ratatui::widgets::{Block, Borders, Padding};

use crate::tui::text::{cell_width_ranges, str_cells};

use super::{PickerKind, PickerMode, PickerView, TagComboboxView, picker_viewport_start};

pub(crate) const GENERIC_PICKER_VIEWPORT_ROWS: usize = 8;
pub(crate) const PROJECT_PICKER_VIEWPORT_ROWS: usize = 10;
pub(crate) const GENERIC_PICKER_WIDTH: u16 = 60;
pub(crate) const LABEL_PICKER_WIDTH: u16 = 68;
pub(crate) const PROJECT_PICKER_WIDTH: u16 = 70;
pub(crate) const TEXT_PANEL_VISIBLE_ROWS: usize = 12;
pub(crate) const TEXT_PANEL_WIDTH: u16 = 60;
pub(crate) const TAG_COMBOBOX_VIEWPORT_ROWS: usize = 7;
pub(crate) const TAG_COMBOBOX_WIDTH: u16 = 68;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PickerLayout {
    pub(crate) area: Rect,
    pub(crate) inner: Rect,
    pub(crate) list: Rect,
    pub(crate) filter: Option<Rect>,
    pub(crate) list_rows: usize,
    pub(crate) visible_start: usize,
    pub(crate) visible_end: usize,
}

impl PickerLayout {
    pub(crate) fn item_at(&self, column: u16, row: u16, indices: &[usize]) -> Option<usize> {
        if !self.list.contains((column, row).into()) {
            return None;
        }
        let position = self.visible_start + usize::from(row - self.list.y);
        (position < self.visible_end).then(|| indices[position])
    }
}

pub(crate) const COMMAND_DIALOG_MAX_WIDTH: u16 = 112;
const COMMAND_VIEWPORT_ROWS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandLayout {
    pub(crate) area: Rect,
    pub(crate) inner: Rect,
    pub(crate) list: Rect,
    pub(crate) visible: std::ops::Range<usize>,
}

impl CommandLayout {
    pub(crate) fn candidate_at(&self, column: u16, row: u16) -> Option<usize> {
        if !self.list.contains((column, row).into()) {
            return None;
        }
        Some(self.visible.start + usize::from(row - self.list.y))
    }
}

pub(crate) fn command_layout(
    terminal_size: Size,
    candidate_count: usize,
    highlighted: Option<usize>,
) -> CommandLayout {
    let selected = highlighted
        .unwrap_or(0)
        .min(candidate_count.saturating_sub(1));
    let offset = selected.saturating_sub(COMMAND_VIEWPORT_ROWS - 1);
    let rows = candidate_count
        .saturating_sub(offset)
        .min(COMMAND_VIEWPORT_ROWS);
    let area = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        COMMAND_DIALOG_MAX_WIDTH,
        rows as u16 + 3 + u16::from(candidate_count > 0),
    );
    let inner = dialog_inner_area(area);
    let list = Rect::new(
        inner.x,
        inner.y + inner.height.min(1),
        inner.width,
        (rows as u16).min(inner.height.saturating_sub(1)),
    );
    CommandLayout {
        area,
        inner,
        list,
        visible: offset..offset + usize::from(list.height),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfirmLayout {
    pub(crate) area: Rect,
    pub(crate) inner: Rect,
    pub(crate) hint_row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextPanelLayout {
    pub(crate) area: Rect,
    pub(crate) inner: Rect,
    pub(crate) visible_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TagComboboxLayout {
    pub(crate) area: Rect,
    pub(crate) inner: Rect,
    pub(crate) chip_start: u16,
    pub(crate) input_row: u16,
    pub(crate) list_start: u16,
    pub(crate) hint_row: u16,
    pub(crate) viewport_rows: usize,
    pub(crate) visible_start: usize,
}

pub(crate) fn dialog_area(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width.min(area.width.saturating_sub(2)))])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(
        height.min(area.height.saturating_sub(2)),
    )])
    .flex(Flex::Center)
    .areas(area);
    area
}

pub(crate) fn dialog_inner_area(area: Rect) -> Rect {
    Block::new()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .inner(area)
}

pub(crate) fn picker_row_count(visible_count: usize, viewport_rows: usize) -> usize {
    visible_count.clamp(1, viewport_rows)
}

pub(crate) fn picker_layout(state: &PickerView, terminal_size: Size) -> PickerLayout {
    let project_picker = matches!(
        state.kind,
        PickerKind::AddTaskProject
            | PickerKind::EditProject
            | PickerKind::ScopeProject
            | PickerKind::ProjectPathProject
            | PickerKind::RenameProject
            | PickerKind::DeleteProject
    );
    let viewport_rows = if project_picker {
        PROJECT_PICKER_VIEWPORT_ROWS
    } else {
        GENERIC_PICKER_VIEWPORT_ROWS
    };
    let row_count = if project_picker {
        state.items.len()
    } else {
        state.visible_indices.len()
    };
    let label_picker = state.kind == PickerKind::LabelAdministration;
    let filtering = state.mode == PickerMode::Filter;
    let list_start = if project_picker || label_picker {
        1 + u16::from(filtering)
    } else if filtering {
        2
    } else {
        0
    };
    let list_rows = if project_picker || label_picker || state.kind == PickerKind::SwitchWorkspace {
        picker_row_count(row_count, viewport_rows)
    } else {
        row_count.min(viewport_rows)
    };
    let height = list_rows as u16 + list_start + 4;
    let width = if project_picker {
        PROJECT_PICKER_WIDTH
    } else if label_picker {
        LABEL_PICKER_WIDTH
    } else {
        GENERIC_PICKER_WIDTH
    };
    let area = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        width,
        height,
    );
    let inner = dialog_inner_area(area);
    let list = Rect::new(
        inner.x,
        inner.y + list_start.min(inner.height),
        inner.width,
        (list_rows as u16).min(inner.height.saturating_sub(list_start)),
    );
    let visible_start = picker_visible_start(state, usize::from(list.height).max(1));
    PickerLayout {
        area,
        inner,
        list,
        filter: (filtering || project_picker || label_picker).then_some(Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.min(1),
        )),
        list_rows,
        visible_start,
        visible_end: (visible_start + usize::from(list.height)).min(state.visible_indices.len()),
    }
}

fn picker_visible_start(state: &PickerView, viewport_rows: usize) -> usize {
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    picker_viewport_start(
        state.scroll,
        selected_position,
        state.visible_indices.len(),
        viewport_rows,
    )
}

pub(crate) fn confirm_layout(terminal_size: Size, prompt: &str) -> ConfirmLayout {
    let width = confirm_width(terminal_size.width, prompt);
    let prompt_rows = cell_width_ranges(prompt, width.saturating_sub(4) as usize).len();
    let height = prompt_rows.saturating_add(4) as u16;
    let area = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        width,
        height,
    );
    ConfirmLayout {
        area,
        inner: dialog_inner_area(area),
        hint_row: prompt_rows.saturating_add(1) as u16,
    }
}

pub(crate) fn confirm_width(frame_width: u16, prompt: &str) -> u16 {
    let prompt_width = str_cells(prompt).saturating_add(4).min(u16::MAX as usize) as u16;
    prompt_width
        .clamp(32, 80)
        .min(frame_width.saturating_sub(4).max(32))
}

pub(crate) fn text_panel_scroll_cap(line_count: usize) -> u16 {
    line_count
        .saturating_sub(TEXT_PANEL_VISIBLE_ROWS)
        .min(u16::MAX as usize) as u16
}

pub(crate) fn tag_combobox_layout(
    state: &TagComboboxView,
    terminal_size: Size,
) -> TagComboboxLayout {
    let height = TAG_COMBOBOX_VIEWPORT_ROWS.saturating_add(6) as u16;
    let area = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        TAG_COMBOBOX_WIDTH,
        height,
    );
    TagComboboxLayout {
        area,
        inner: dialog_inner_area(area),
        chip_start: 0,
        input_row: 0,
        list_start: 2,
        hint_row: height.saturating_sub(3),
        viewport_rows: TAG_COMBOBOX_VIEWPORT_ROWS,
        visible_start: state
            .visible_indices
            .iter()
            .position(|index| *index == state.highlighted)
            .unwrap_or(0)
            .saturating_sub(TAG_COMBOBOX_VIEWPORT_ROWS.saturating_sub(1)),
    }
}

pub(crate) fn text_panel_layout(terminal_size: Size, line_count: usize) -> TextPanelLayout {
    let content_rows = line_count.clamp(1, TEXT_PANEL_VISIBLE_ROWS);
    let height = (content_rows as u16).saturating_add(4).min(16);
    let area = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        TEXT_PANEL_WIDTH,
        height,
    );
    TextPanelLayout {
        area,
        inner: dialog_inner_area(area),
        visible_rows: TEXT_PANEL_VISIBLE_ROWS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_picker_view(item_count: usize, visible_count: usize) -> PickerView<'static> {
        PickerView {
            kind: PickerKind::ScopeProject,
            title: "Scope: project".to_string(),
            filter: String::new(),
            filter_cursor: 0,
            items: Box::leak(
                (0..item_count)
                    .map(|index| super::super::PickerItem {
                        label: format!("Project {index}"),
                        value: index.to_string(),
                        selected: false,
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Filter,
            visible_indices: (0..visible_count).collect(),
        }
    }

    #[test]
    fn project_picker_height_tracks_items_up_to_viewport_limit() {
        let terminal = Size::new(120, 50);

        assert_eq!(
            picker_layout(&project_picker_view(0, 0), terminal)
                .area
                .height,
            7
        );
        assert_eq!(
            picker_layout(&project_picker_view(7, 7), terminal)
                .area
                .height,
            13
        );
        assert_eq!(
            picker_layout(&project_picker_view(20, 20), terminal)
                .area
                .height,
            16
        );
    }

    #[test]
    fn project_picker_height_stays_stable_while_filtering() {
        let terminal = Size::new(120, 50);

        let unfiltered = picker_layout(&project_picker_view(7, 7), terminal);
        let one_match = picker_layout(&project_picker_view(7, 1), terminal);
        let no_matches = picker_layout(&project_picker_view(7, 0), terminal);

        assert_eq!(one_match.area.height, unfiltered.area.height);
        assert_eq!(no_matches.area.height, unfiltered.area.height);
    }

    #[test]
    fn generic_picker_modes_share_exact_row_mapping() {
        for mode in [PickerMode::Navigate, PickerMode::Filter] {
            let mut view = project_picker_view(12, 12);
            view.kind = PickerKind::Generic;
            view.mode = mode;
            view.visible_indices = vec![1, 4, 7, 10];
            view.selected = 10;
            let layout = picker_layout(&view, Size::new(80, 24));
            assert_eq!(
                layout.list.y - layout.inner.y,
                if mode == PickerMode::Filter { 2 } else { 0 }
            );
            for (offset, index) in view.visible_indices.iter().enumerate() {
                assert_eq!(
                    layout.item_at(
                        layout.list.x,
                        layout.list.y + offset as u16,
                        &view.visible_indices
                    ),
                    Some(*index)
                );
            }
            assert_eq!(
                layout.item_at(layout.list.x, layout.list.bottom(), &view.visible_indices),
                None
            );
            assert_eq!(layout.filter.is_some(), mode == PickerMode::Filter);
        }
    }

    #[test]
    fn command_layout_clips_mouse_targets_to_content() {
        for width in [0, 1, 20, 72, 120] {
            for height in [0, 1, 4, 7, 30] {
                for count in [0, 1, 20] {
                    let layout = command_layout(Size::new(width, height), count, Some(19));
                    assert!(layout.visible.end <= count);
                    for row in 0..height {
                        for column in 0..width {
                            if let Some(index) = layout.candidate_at(column, row) {
                                assert!(layout.inner.contains((column, row).into()));
                                assert!(layout.visible.contains(&index));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn confirm_width_counts_wide_prompt_cells() {
        assert_eq!(confirm_width(120, &"한".repeat(20)), 44);
    }
}

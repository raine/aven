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
    pub(crate) list_start: u16,
    pub(crate) viewport_rows: usize,
    pub(crate) visible_start: usize,
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
    let list_rows = picker_row_count(row_count, viewport_rows);
    if project_picker {
        let height = (list_rows as u16).saturating_add(if state.mode == PickerMode::Filter {
            6
        } else {
            5
        });
        let area = dialog_area(
            Rect::new(0, 0, terminal_size.width, terminal_size.height),
            PROJECT_PICKER_WIDTH,
            height,
        );
        return PickerLayout {
            area,
            inner: dialog_inner_area(area),
            list_start: if state.mode == PickerMode::Filter {
                2
            } else {
                1
            },
            viewport_rows: list_rows,
            visible_start: picker_visible_start(state, list_rows),
        };
    }

    let label_picker = state.kind == PickerKind::LabelAdministration;
    let height = (list_rows as u16).saturating_add(if label_picker { 7 } else { 6 });
    let area = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        if label_picker {
            LABEL_PICKER_WIDTH
        } else {
            GENERIC_PICKER_WIDTH
        },
        height,
    );
    PickerLayout {
        area,
        inner: dialog_inner_area(area),
        list_start: if label_picker {
            if state.mode == PickerMode::Filter {
                2
            } else {
                1
            }
        } else {
            2
        },
        viewport_rows: GENERIC_PICKER_VIEWPORT_ROWS,
        visible_start: picker_visible_start(state, GENERIC_PICKER_VIEWPORT_ROWS),
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
    fn confirm_width_counts_wide_prompt_cells() {
        assert_eq!(confirm_width(120, &"한".repeat(20)), 44);
    }
}

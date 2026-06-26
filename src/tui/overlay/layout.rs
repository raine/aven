use ratatui::layout::{Constraint, Flex, Layout, Rect, Size};
use ratatui::widgets::{Block, Borders, Padding};

use crate::tui::text::char_count_ranges;

use super::{OverlayRoute, PickerView, picker_viewport_start};

pub(crate) const GENERIC_PICKER_VIEWPORT_ROWS: usize = 8;
pub(crate) const PROJECT_PICKER_VIEWPORT_ROWS: usize = 10;
pub(crate) const GENERIC_PICKER_WIDTH: u16 = 60;
pub(crate) const PROJECT_PICKER_WIDTH: u16 = 70;
pub(crate) const TEXT_PANEL_VISIBLE_ROWS: usize = 12;
pub(crate) const TEXT_PANEL_WIDTH: u16 = 60;

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

pub(crate) fn picker_layout(state: &PickerView, terminal_size: Size) -> PickerLayout {
    if project_picker_layout(state.route) {
        let height = (PROJECT_PICKER_VIEWPORT_ROWS as u16).saturating_add(6);
        let area = dialog_area(
            Rect::new(0, 0, terminal_size.width, terminal_size.height),
            PROJECT_PICKER_WIDTH,
            height,
        );
        return PickerLayout {
            area,
            inner: dialog_inner_area(area),
            list_start: 2,
            viewport_rows: PROJECT_PICKER_VIEWPORT_ROWS,
            visible_start: picker_visible_start(state, PROJECT_PICKER_VIEWPORT_ROWS),
        };
    }

    let visible_count = state.visible_indices.len().max(1);
    let height = (visible_count.min(GENERIC_PICKER_VIEWPORT_ROWS) as u16).saturating_add(6);
    let area = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        GENERIC_PICKER_WIDTH,
        height,
    );
    PickerLayout {
        area,
        inner: dialog_inner_area(area),
        list_start: 2,
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
    let prompt_rows = char_count_ranges(prompt, width.saturating_sub(4) as usize).len();
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
    let prompt_width = prompt.chars().count().saturating_add(4) as u16;
    prompt_width
        .clamp(32, 80)
        .min(frame_width.saturating_sub(4).max(32))
}

pub(crate) fn text_panel_scroll_cap(line_count: usize) -> u16 {
    line_count
        .saturating_sub(TEXT_PANEL_VISIBLE_ROWS)
        .min(u16::MAX as usize) as u16
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

fn project_picker_layout(route: OverlayRoute) -> bool {
    matches!(
        route,
        OverlayRoute::ScopeProject
            | OverlayRoute::EditProject
            | OverlayRoute::AddTaskTitleProject
            | OverlayRoute::DeleteProjectPicker
    )
}

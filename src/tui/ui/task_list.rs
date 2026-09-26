mod cells;
mod hit_test;
mod layout;
mod preview;
mod sizing;
mod source;
mod table;
mod view_model;

pub(crate) use self::hit_test::{TaskListHit, task_at_position, task_status_at_position};
use self::layout::{TaskListAreas, task_list_areas};
use self::source::TaskListSource;
use self::table::render_task_list;
pub(crate) use self::view_model::TaskListView;

use crate::tui::app::Focus;
use crate::tui::list_surface::ListSurface;
use crate::tui::overlay::TextInputView;
use crate::tui::store::TuiStore;
use ratatui::Frame;
use ratatui::layout::Rect;
pub(super) const EPIC_MARKER: &str = "\u{f04ce}";
pub(super) use self::preview::render_task_preview;

pub(crate) fn task_visual_row(store: &TuiStore, task_index: usize) -> Option<usize> {
    store.task_list_view().visual_row_for(task_index)
}

pub(crate) fn task_index_at_visual_row(store: &TuiStore, visual_row: usize) -> Option<usize> {
    store.task_list_view().task_index_at_visual_row(visual_row)
}

pub(crate) fn task_visual_row_count(store: &TuiStore) -> usize {
    store.task_list_view().row_count()
}

pub(super) fn task_list_page_rows(area: Rect, has_tasks: bool) -> usize {
    let table_area = if has_tasks {
        task_list_areas(area).table_area
    } else {
        area
    };
    usize::from(table_area.height.saturating_sub(1).max(1))
}

pub(super) fn render_tasks(
    frame: &mut Frame,
    store: &TuiStore,
    list: &mut ListSurface,
    focus: Focus,
    area: Rect,
    inline_title_editor: Option<&TextInputView>,
) {
    let TaskListAreas {
        table_area,
        preview_area,
    } = if store.tasks.is_empty() {
        TaskListAreas {
            table_area: area,
            preview_area: Rect::default(),
        }
    } else {
        task_list_areas(area)
    };
    let marked_task_ids = list.marked_task_ids().clone();
    render_task_list(
        frame,
        &TaskListSource::from_store(store),
        list.table_state_mut(),
        focus,
        table_area,
        inline_title_editor,
        &marked_task_ids,
    );
    if !store.tasks.is_empty() && preview_area.height > 0 {
        render_task_preview(frame, store, list.selected_task(), preview_area);
    }
}

#[cfg(test)]
pub(super) mod tests;

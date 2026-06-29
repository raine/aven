mod add_task;
mod confirm;
mod database_stats;
mod multiline;
mod picker;
mod search;
mod shared;
mod sync_status;
mod tag_combobox;
mod text_input;
mod text_panel;

pub(super) use add_task::{render_add_task, render_add_task_full_frame};
pub(super) use confirm::render_confirm;
pub(crate) use database_stats::database_stats_scroll_cap;
pub(super) use database_stats::render_database_stats;
pub(super) use multiline::{
    add_task_description_hint_line, add_task_free_text_input_line, add_task_natural_hint_line,
    render_multiline_input,
};
pub(super) use picker::render_picker;
pub(super) use search::{SearchRenderStatus, render_search};
pub(super) use shared::tail_viewport_start;
pub(super) use sync_status::render_sync_status;
pub(super) use tag_combobox::render_tag_combobox;
pub(super) use text_input::render_text_input;
pub(super) use text_panel::render_text_panel;
pub(crate) use text_panel::text_panel_scroll_cap;

#[cfg(test)]
pub(super) use add_task::{
    ADD_TASK_TITLE_PLACEHOLDER, add_task_description_lines, add_task_hint_line,
    add_task_metadata_title, add_task_priority_hint_line, add_task_status_hint_line,
    add_task_title_input_line,
};

#[cfg(test)]
pub(super) use confirm::confirm_hint_line;

#[cfg(test)]
pub(super) use multiline::{
    CONFLICT_MANUAL_BODY_PLACEHOLDER, add_note_input_line, add_task_description_input_line,
    description_editor_lines, description_input_line, multiline_hint_line,
};

#[cfg(test)]
pub(super) use sync_status::sync_status_lines_for_test;

#[cfg(test)]
pub(super) use text_input::{
    ADD_LABEL_NAME_PLACEHOLDER, ADD_PROJECT_NAME_PLACEHOLDER, CONFLICT_MANUAL_VALUE_PLACEHOLDER,
    RENAME_PROJECT_NAME_PLACEHOLDER, placeholder_text_input_line,
};

#[cfg(test)]
mod tests;

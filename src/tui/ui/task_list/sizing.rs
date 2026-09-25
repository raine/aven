use super::cells::{EpicSelectionContext, is_deferred, metadata_cell};
use super::source::TaskListSource;
use super::view_model::TaskListRow;
use crate::config::TableColumn;
use crate::query::TaskListItem;
use crate::queue::now_seconds;
use crate::tui::store::TaskListRenderMode;
use ratatui::layout::Constraint;
use unicode_width::UnicodeWidthStr;

/// Compact due labels are at most five columns wide; the sixth is the cell gutter.
pub(super) const DUE_COLUMN_WIDTH: u16 = 6;

pub(super) fn task_list_columns(source: &TaskListSource<'_>, narrow: bool) -> [Constraint; 9] {
    task_list_columns_for_tasks(
        source,
        narrow,
        &source.tasks.iter().collect::<Vec<_>>(),
        EpicSelectionContext::default(),
    )
}

pub(super) fn task_list_columns_for_tasks(
    source: &TaskListSource<'_>,
    narrow: bool,
    label_tasks: &[&TaskListItem],
    epic_selection: EpicSelectionContext<'_>,
) -> [Constraint; 9] {
    let epics = source.view_state.render_mode() == TaskListRenderMode::Epics;
    let project_width = if epics && narrow {
        0
    } else {
        project_column_width(source.tasks, narrow)
    };
    let label_width = if epics {
        if narrow { 18 } else { 24 }
    } else {
        label_column_width_from_task_refs(label_tasks, narrow)
    };
    let metadata_width = metadata_column_width_from_task_refs(
        label_tasks,
        epic_selection,
        source.view_state.render_mode() == TaskListRenderMode::Flat,
    );
    let priority_width = priority_column_width_from_tasks(source.tasks);
    let ref_width = if epics { 14 } else { 12 };
    TableColumn::ALL.map(|column| match column {
        TableColumn::Ref => Constraint::Length(ref_width),
        TableColumn::Title => Constraint::Fill(1),
        TableColumn::Labels => Constraint::Length(label_width),
        TableColumn::Metadata => Constraint::Length(metadata_width),
        TableColumn::Project => Constraint::Length(project_width),
        TableColumn::Status => Constraint::Length(status_column_width(source)),
        TableColumn::Priority => Constraint::Length(priority_width),
        TableColumn::Time => Constraint::Length(5),
        TableColumn::Due => Constraint::Length(DUE_COLUMN_WIDTH),
    })
}

/// Width of the status column, including the gutter that follows it.
pub(super) fn status_column_width(source: &TaskListSource<'_>) -> u16 {
    if source.table.compact_status { 2 } else { 10 }
}

pub(super) fn project_column_width(tasks: &[TaskListItem], narrow: bool) -> u16 {
    let max_width = if narrow { 14 } else { 18 };
    tasks
        .iter()
        .map(|item| item.task.project_key.width() as u16 + 2)
        .max()
        .unwrap_or(9)
        .max(9)
        .min(max_width)
}

pub(super) fn visible_task_items<'a>(
    source: &TaskListSource<'a>,
    visible_rows: &[(usize, &TaskListRow)],
) -> Vec<&'a TaskListItem> {
    visible_rows
        .iter()
        .filter_map(|(_, row)| match row {
            TaskListRow::Group(_) => None,
            TaskListRow::Task { task_index } | TaskListRow::EpicChild { task_index, .. } => {
                source.tasks.get(*task_index)
            }
        })
        .collect()
}

#[cfg(test)]
pub(super) fn label_column_width_from_tasks(tasks: &[TaskListItem], narrow: bool) -> u16 {
    let tasks = tasks.iter().collect::<Vec<_>>();
    label_column_width_from_task_refs(&tasks, narrow)
}

pub(super) fn label_column_width_from_task_refs(tasks: &[&TaskListItem], narrow: bool) -> u16 {
    if narrow {
        return 0;
    }
    tasks
        .iter()
        .filter(|item| !item.labels.is_empty())
        .map(|item| {
            let first = item.labels.first().map_or(0, |label| label.width());
            let more = item.labels.len().saturating_sub(1);
            let summary_width = if more == 0 {
                first
            } else {
                first + more.to_string().len() + 2
            };
            summary_width as u16 + 2
        })
        .max()
        .unwrap_or(0)
        .min(18)
}

#[cfg(test)]
pub(super) fn metadata_column_width_from_tasks(tasks: &[TaskListItem]) -> u16 {
    let tasks = tasks.iter().collect::<Vec<_>>();
    metadata_column_width_from_task_refs(&tasks, EpicSelectionContext::default(), false)
}

pub(super) fn metadata_column_width_from_task_refs(
    tasks: &[&TaskListItem],
    epic_selection: EpicSelectionContext<'_>,
    mark_deferred: bool,
) -> u16 {
    let now = now_seconds();
    let width = tasks
        .iter()
        .map(|item| {
            metadata_cell(
                item,
                epic_selection,
                mark_deferred && is_deferred(item, now),
            )
            .to_string()
            .chars()
            .count() as u16
        })
        .max()
        .unwrap_or(0);
    if width == 0 { 0 } else { width + 2 }
}

pub(super) fn priority_column_width_from_tasks(tasks: &[TaskListItem]) -> u16 {
    if tasks
        .iter()
        .any(|item| item.task.priority.as_str() != "none")
    {
        3
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;

    #[test]
    fn label_column_width_uses_visible_task_labels() {
        let mut hidden_wide = task_list_item_with_id("zz hidden wide label", "task-4");
        hidden_wide.labels = vec!["very-wide-label".to_string()];
        let list = TaskListFixture::new(vec![
            task_list_item_with_id("aa visible plain one", "task-1"),
            task_list_item_with_id("bb visible plain two", "task-2"),
            task_list_item_with_id("cc visible plain three", "task-3"),
            hidden_wide,
        ]);
        let area = Rect::new(0, 0, 100, 4);
        let mut table_state = TableState::default();

        let top_model = build_task_list_render_model(
            &list.source(),
            &mut table_state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::new(),
        );

        assert_eq!(top_model.layout.widths()[TableColumn::Labels as usize], 0);

        table_state.select(Some(3));
        let scrolled_model = build_task_list_render_model(
            &list.source(),
            &mut table_state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::new(),
        );

        assert_eq!(
            scrolled_model.layout.widths()[TableColumn::Labels as usize],
            16
        );
    }

    #[test]
    fn label_column_width_collapses_without_visible_labels() {
        let tasks = vec![task_list_item("plain"), task_list_item("also plain")];

        assert_eq!(label_column_width_from_tasks(&tasks, false), 0);
    }

    #[test]
    fn label_column_width_reserves_lane_for_visible_labels() {
        let mut task = task_list_item("labeled");
        task.labels = vec!["search".to_string(), "ux".to_string()];

        assert_eq!(label_column_width_from_tasks(&[task], false), 11);
    }

    #[test]
    fn label_column_width_counts_wide_label_cells() {
        let mut task = task_list_item("labeled");
        task.labels = vec!["한글".to_string()];

        assert_eq!(label_column_width_from_tasks(&[task], false), 6);
    }

    #[test]
    fn project_column_width_counts_wide_key_cells() {
        let mut task = task_list_item("task");
        task.task.project_key = "프로젝트".to_string();

        assert_eq!(project_column_width(&[task], false), 10);
    }

    #[test]
    fn label_column_width_collapses_in_narrow_layout() {
        let mut task = task_list_item("labeled");
        task.labels = vec!["search".to_string()];

        assert_eq!(label_column_width_from_tasks(&[task], true), 0);
    }

    #[test]
    fn metadata_column_width_collapses_without_metadata() {
        let tasks = vec![task_list_item("plain"), task_list_item("also plain")];

        assert_eq!(metadata_column_width_from_tasks(&tasks), 0);
    }

    #[test]
    fn metadata_column_width_uses_given_task_refs() {
        let plain = task_list_item("plain");
        let mut documented = task_list_item("documented");
        documented.notes = vec![crate::query::TaskNote {
            id: "note-id".to_string(),
            body: "one".to_string(),
            created_at: "001".to_string(),
        }];
        documented.has_notes = true;
        let visible_tasks = vec![&plain];
        let all_tasks = vec![&plain, &documented];

        assert_eq!(
            metadata_column_width_from_task_refs(
                &visible_tasks,
                EpicSelectionContext::default(),
                false
            ),
            0
        );
        assert_eq!(
            metadata_column_width_from_task_refs(
                &all_tasks,
                EpicSelectionContext::default(),
                false
            ),
            3
        );
    }

    #[test]
    fn metadata_column_width_reserves_lane_for_deferred_marker() {
        let mut task = task_list_item("deferred");
        task.task.available_at = Some("2999-01-01T00:00:00Z".to_string());

        assert_eq!(
            metadata_column_width_from_task_refs(&[&task], EpicSelectionContext::default(), true),
            3
        );
        assert_eq!(
            metadata_column_width_from_task_refs(&[&task], EpicSelectionContext::default(), false),
            0
        );
    }

    #[test]
    fn metadata_column_width_reserves_lane_for_metadata() {
        let mut task = task_list_item("documented");
        task.notes = vec![crate::query::TaskNote {
            id: "note-id".to_string(),
            body: "one".to_string(),
            created_at: "001".to_string(),
        }];
        task.has_notes = true;

        assert_eq!(metadata_column_width_from_tasks(&[task]), 3);
    }

    #[test]
    fn metadata_column_width_reserves_lane_for_epics() {
        let mut task = task_list_item("epic");
        task.task.is_epic = true;

        assert_eq!(metadata_column_width_from_tasks(&[task]), 3);
    }

    #[test]
    fn priority_column_width_collapses_without_priority() {
        let tasks = vec![task_list_item("plain"), task_list_item("also plain")];

        assert_eq!(priority_column_width_from_tasks(&tasks), 0);
    }

    #[test]
    fn priority_column_width_reserves_lane_for_priority() {
        let mut task = task_list_item("prioritized");
        task.task.priority = TaskPriority::High;

        assert_eq!(priority_column_width_from_tasks(&[task]), 3);
    }
}

use std::collections::BTreeSet;

use anyhow::Result;

use crate::choices::TaskStatus;
use crate::labels::normalize_label;
use crate::query::TaskListItem;
use crate::tui::app::{App, FooterChoiceMode};
use crate::tui::overlay::{
    MultilineInputState, OverlayRoute, OverlayState, PickerItem, SearchPurpose, SearchState,
};
use crate::tui::platform::edit_text_externally;

pub(crate) const EDIT_TITLE_TITLE: &str = "Edit title";
pub(crate) const EDIT_DESCRIPTION_TITLE: &str = "Edit description";
pub(crate) const EDIT_PROJECT_TITLE: &str = "Edit project";
pub(crate) const EDIT_LABELS_TITLE: &str = "Edit task: labels";
pub(crate) const REMOVE_DEPENDENCY_TITLE: &str = "Remove dependency";

impl App {
    pub(super) async fn update_status(&mut self, status: &'static str) -> Result<()> {
        let task_ids = self.marked_task_ids_in_view();
        let result = if task_ids.is_empty() {
            let preserve_done_detail = status == "done"
                && (self.store.view_state.view == crate::tui::store::TaskView::Columns
                    || self.detail_context
                    || matches!(self.overlay, Some(OverlayState::Detail { .. })));
            if preserve_done_detail {
                self.store
                    .update_status_preserving_task(self.widgets.table.selected(), status)
                    .await?
            } else {
                self.store
                    .update_status(self.widgets.table.selected(), status)
                    .await?
            }
        } else {
            self.store
                .update_status_for_tasks(self.widgets.table.selected(), &task_ids, status)
                .await?
        };
        if let Some(result) = result {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn move_tasks_by_column(&mut self, delta: isize) -> Result<()> {
        if self.store.view_state.view != crate::tui::store::TaskView::Columns {
            self.set_info("column moves are available in Columns view");
            return Ok(());
        }
        let targets = self.column_move_targets();
        if targets.is_empty() {
            self.set_info("no selected task to move");
            return Ok(());
        }
        let mut changes = Vec::with_capacity(targets.len());
        for item in &targets {
            let Some(status) = crate::tui::columns::adjacent_lane_entry_status(
                &self.store.task_columns,
                item.task.status,
                delta,
            ) else {
                self.set_info(if delta < 0 {
                    "already at first column"
                } else {
                    "already at last column"
                });
                return Ok(());
            };
            changes.push((item.task.id.clone(), status.as_str().to_string()));
        }
        self.apply_column_moves(changes).await
    }

    pub(super) fn begin_move_to_column(&mut self) {
        if self.store.view_state.view != crate::tui::store::TaskView::Columns {
            self.set_info("column moves are available in Columns view");
            return;
        }
        let targets = self.column_move_targets();
        if targets.is_empty() {
            self.set_info("no selected task to move");
            return;
        }
        let selected_lane = if targets.len() == 1 {
            crate::tui::columns::lane_index_for_status(
                &self.store.task_columns,
                targets[0].task.status,
            )
        } else {
            None
        };
        let items = self
            .store
            .task_columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| {
                let status =
                    crate::tui::columns::lane_entry_status(&self.store.task_columns, index)?;
                Some(PickerItem {
                    label: format!("{} → {}", column.name, status.as_str()),
                    value: status.as_str().to_string(),
                    selected: selected_lane == Some(index),
                })
            })
            .collect();
        let title = if targets.len() == 1 {
            "Move to column".to_string()
        } else {
            format!("Move to column: {} marked tasks", targets.len())
        };
        self.open_picker_overlay(OverlayRoute::MoveToColumn, title, items, false);
    }

    pub(super) async fn move_tasks_to_column(&mut self, status: String) -> Result<()> {
        let target_status = TaskStatus::parse(&status)?;
        let Some(target_lane) =
            crate::tui::columns::lane_index_for_status(&self.store.task_columns, target_status)
        else {
            self.set_warning("column is unavailable");
            return Ok(());
        };
        let changes = self
            .column_move_targets()
            .into_iter()
            .filter_map(|item| {
                let source_lane = crate::tui::columns::lane_index_for_status(
                    &self.store.task_columns,
                    item.task.status,
                )?;
                (source_lane != target_lane).then(|| (item.task.id, status.clone()))
            })
            .collect::<Vec<_>>();
        if changes.is_empty() {
            self.set_info("tasks are already in that column");
            return Ok(());
        }
        self.apply_column_moves(changes).await
    }

    fn column_move_targets(&self) -> Vec<TaskListItem> {
        let task_ids = self.marked_task_ids_in_view();
        if task_ids.is_empty() {
            return self
                .store
                .selected_task(self.widgets.table.selected())
                .cloned()
                .into_iter()
                .collect();
        }
        let task_ids = task_ids.iter().collect::<BTreeSet<_>>();
        self.store
            .tasks
            .iter()
            .filter(|item| task_ids.contains(&item.task.id))
            .cloned()
            .collect()
    }

    async fn apply_column_moves(&mut self, changes: Vec<(String, String)>) -> Result<()> {
        let result = self
            .store
            .update_status_changes_for_tasks(self.widgets.table.selected(), &changes)
            .await?;
        if let Some(result) = result {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to move");
        }
        Ok(())
    }

    pub(super) async fn set_exact_priority(&mut self, priority: &'static str) -> Result<()> {
        let task_ids = self.marked_task_ids_in_view();
        let result = if task_ids.is_empty() {
            self.store
                .set_exact_priority(self.widgets.table.selected(), priority)
                .await?
        } else {
            self.store
                .set_exact_priority_for_tasks(self.widgets.table.selected(), &task_ids, priority)
                .await?
        };
        if let Some(result) = result {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn update_priority(&mut self, reverse: bool) -> Result<()> {
        let task_ids = self.marked_task_ids_in_view();
        let result = if task_ids.is_empty() {
            self.store
                .update_priority(self.widgets.table.selected(), reverse)
                .await?
        } else {
            self.store
                .update_priority_for_tasks(self.widgets.table.selected(), &task_ids, reverse)
                .await?
        };
        if let Some(result) = result {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn update_deleted(&mut self, deleted: bool) -> Result<()> {
        let task_ids = self.marked_task_ids_in_view();
        let result = if task_ids.is_empty() {
            self.store
                .update_deleted(self.widgets.table.selected(), deleted)
                .await?
        } else {
            self.store
                .update_deleted_for_tasks(self.widgets.table.selected(), &task_ids, deleted)
                .await?
        };
        if let Some(result) = result {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn undo_last(&mut self) -> Result<()> {
        let selected =
            if self.detail_context || matches!(self.overlay, Some(OverlayState::Detail { .. })) {
                None
            } else {
                self.widgets.table.selected()
            };
        match self.store.undo_last(selected).await? {
            Some(result) => self.apply_mutation_result(result),
            None => self.set_info("nothing to undo"),
        }
        Ok(())
    }

    fn guard_selected_task(&mut self) -> Option<usize> {
        self.pending_shortcut.clear();
        let index = self.widgets.table.selected();
        if index.is_some_and(|i| self.store.selected_task(Some(i)).is_some()) {
            index
        } else {
            self.set_info("no selected task to edit");
            None
        }
    }

    fn apply_edit_mutation<F>(
        &mut self,
        result: Result<Option<crate::tui::store::MutationMessage>>,
        on_error: F,
    ) where
        F: FnOnce(&mut Self),
    {
        match result {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_info("no selected task to edit"),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                on_error(self);
            }
        }
    }

    pub(super) fn begin_status_picker(&mut self) {
        if self.marked_task_ids_in_view().is_empty() && self.guard_selected_task().is_none() {
            return;
        }
        self.footer_choice_mode = Some(FooterChoiceMode::Status);
    }

    pub(super) fn begin_edit_title(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let title = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .title
            .clone();
        self.open_edit_title_overlay(title);
    }

    fn open_edit_title_overlay(&mut self, input: String) {
        self.detail_context_scroll = 0;
        self.overlay = Some(OverlayState::text_input(
            OverlayRoute::EditTitle,
            EDIT_TITLE_TITLE,
            "",
            input,
        ));
    }

    fn open_edit_description_overlay(&mut self, value: String) {
        self.overlay = Some(OverlayState::multiline_input(
            OverlayRoute::EditDescription,
            EDIT_DESCRIPTION_TITLE,
            "",
            value,
        ));
    }

    pub(super) fn begin_edit_description(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let description = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .description
            .clone();
        self.open_edit_description_overlay(description);
    }

    pub(super) fn begin_edit_project(&mut self) {
        let task_ids = self.marked_task_ids_in_view();
        if !task_ids.is_empty() {
            let items = self.store.existing_project_picker_items("");
            let count = task_ids.len();
            self.open_picker_overlay(
                OverlayRoute::EditProject,
                format!("Edit project: {count} marked tasks"),
                items,
                false,
            );
            return;
        }
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let selected = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .project_key
            .as_str();
        let items = self.store.existing_project_picker_items(selected);
        self.open_picker_overlay(OverlayRoute::EditProject, EDIT_PROJECT_TITLE, items, false);
    }

    pub(super) fn begin_edit_priority(&mut self) {
        let task_ids = self.marked_task_ids_in_view();
        if !task_ids.is_empty() {
            let items = self.store.priority_picker_items("");
            let count = task_ids.len();
            self.open_picker_overlay(
                OverlayRoute::EditPriority,
                format!("Edit priority: {count} marked tasks"),
                items,
                false,
            );
            return;
        }
        if self.guard_selected_task().is_none() {
            return;
        }
        self.footer_choice_mode = Some(FooterChoiceMode::Priority);
    }

    pub(super) fn begin_edit_labels(&mut self) {
        let task_ids = self.marked_task_ids_in_view();
        if !task_ids.is_empty() {
            self.open_edit_labels_multi(task_ids);
            return;
        }
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let labels = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .labels
            .clone();
        self.overlay = Some(OverlayState::tag_combobox(
            OverlayRoute::EditLabels,
            EDIT_LABELS_TITLE,
            self.store.labels.clone(),
            labels,
        ));
    }

    pub(super) async fn begin_add_dependency(&mut self) -> Result<()> {
        let Some(index) = self.guard_selected_task() else {
            return Ok(());
        };
        let item = self.store.selected_task(Some(index)).unwrap();
        let task_id = item.task.id.clone();
        let display_ref = item.display_ref.clone();
        self.clear_live_search_preview();
        self.overlay = Some(OverlayState::Search(SearchState::for_purpose(
            SearchPurpose::AddDependency {
                task_id,
                display_ref,
            },
        )));
        Ok(())
    }

    pub(super) fn begin_remove_dependency(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let items = self.store.selected_dependency_picker_items(Some(index));
        self.open_picker_overlay(
            OverlayRoute::RemoveDependency,
            REMOVE_DEPENDENCY_TITLE,
            items,
            false,
        );
    }

    pub(super) async fn submit_edit_status(&mut self, status: String) -> Result<()> {
        let task_ids = self.marked_task_ids_in_view();
        let result = if task_ids.is_empty() {
            let preserve_done_detail = status == "done"
                && (self.store.view_state.view == crate::tui::store::TaskView::Columns
                    || self.detail_context
                    || matches!(self.overlay, Some(OverlayState::Detail { .. })));
            if preserve_done_detail {
                self.store
                    .update_status_preserving_task(self.widgets.table.selected(), &status)
                    .await
            } else {
                self.store
                    .update_status(self.widgets.table.selected(), &status)
                    .await
            }
        } else {
            self.store
                .update_status_for_tasks(self.widgets.table.selected(), &task_ids, &status)
                .await
        };
        self.apply_edit_mutation(result, |app| app.begin_status_picker());
        Ok(())
    }

    pub(super) async fn submit_edit_title(&mut self, value: String) -> Result<()> {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            self.set_warning("task title is required");
            self.open_edit_title_overlay(value);
            return Ok(());
        }
        match self
            .store
            .update_title(self.widgets.table.selected(), trimmed)
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_info("no selected task to edit"),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.open_edit_title_overlay(value);
            }
        }
        Ok(())
    }

    pub(super) async fn submit_edit_description(&mut self, value: String) -> Result<()> {
        let result = self
            .store
            .update_description(self.widgets.table.selected(), value.clone())
            .await;
        self.apply_edit_mutation(result, |app| app.open_edit_description_overlay(value));
        Ok(())
    }

    pub(super) async fn submit_edit_project(&mut self, project: String) -> Result<()> {
        let task_ids = self.marked_task_ids_in_view();
        let result = if task_ids.is_empty() {
            self.store
                .update_project(self.widgets.table.selected(), project)
                .await
        } else {
            self.store
                .update_project_for_tasks(self.widgets.table.selected(), &task_ids, project)
                .await
        };
        self.apply_edit_mutation(result, |app| app.begin_edit_project());
        Ok(())
    }

    pub(super) async fn submit_edit_priority(&mut self, priority: String) -> Result<()> {
        let task_ids = self.marked_task_ids_in_view();
        let result = if task_ids.is_empty() {
            self.store
                .set_exact_priority(self.widgets.table.selected(), &priority)
                .await
        } else {
            self.store
                .set_exact_priority_for_tasks(self.widgets.table.selected(), &task_ids, &priority)
                .await
        };
        self.apply_edit_mutation(result, |app| app.begin_edit_priority());
        Ok(())
    }

    pub(super) async fn submit_edit_labels(&mut self, labels: Vec<String>) -> Result<()> {
        for label in &labels {
            let label = normalize_label(label);
            if !self.store.labels.contains(&label)
                && let Err(error) = self.store.create_label(label).await
            {
                self.set_error(format!("{error:#}"));
                self.begin_edit_labels();
                return Ok(());
            }
        }
        let result = self
            .store
            .update_labels(self.widgets.table.selected(), labels)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_labels());
        Ok(())
    }

    fn open_edit_labels_multi(&mut self, task_ids: Vec<String>) {
        let labels = self.store.union_labels_for_tasks(&task_ids);
        let count = task_ids.len();
        self.overlay = Some(OverlayState::tag_combobox(
            OverlayRoute::EditLabelsMulti,
            format!("Edit labels: {count} marked tasks"),
            self.store.labels.clone(),
            labels,
        ));
    }

    pub(super) async fn submit_edit_labels_multi(&mut self, labels: Vec<String>) -> Result<()> {
        for label in &labels {
            let label = normalize_label(label);
            if !self.store.labels.contains(&label)
                && let Err(error) = self.store.create_label(label).await
            {
                self.set_error(format!("{error:#}"));
                self.begin_edit_labels();
                return Ok(());
            }
        }
        let selected = self.widgets.table.selected();
        let task_ids = self.marked_task_ids_in_view();
        let result = self
            .store
            .update_labels_for_tasks(selected, &task_ids, labels)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_labels());
        Ok(())
    }

    pub(super) fn toggle_mark_selected(&mut self) {
        self.pending_shortcut.clear();
        let Some(index) = self.widgets.table.selected() else {
            self.set_info("no selected task to mark");
            return;
        };
        let Some(id) = self
            .store
            .selected_task(Some(index))
            .map(|item| item.task.id.clone())
        else {
            self.set_info("no selected task to mark");
            return;
        };
        if !self.widgets.marked_task_ids.insert(id.clone()) {
            self.widgets.marked_task_ids.remove(&id);
        }
    }

    pub(super) fn toggle_mark_all_in_view(&mut self) {
        self.pending_shortcut.clear();
        let visible = self
            .store
            .tasks
            .iter()
            .map(|item| item.task.id.clone())
            .collect::<BTreeSet<_>>();
        if visible.is_empty() {
            self.set_info("no visible tasks to mark");
            return;
        }
        if visible
            .iter()
            .all(|id| self.widgets.marked_task_ids.contains(id))
        {
            self.widgets
                .marked_task_ids
                .retain(|id| !visible.contains(id));
        } else {
            self.widgets.marked_task_ids.extend(visible);
        }
    }

    pub(super) fn clear_marks(&mut self) {
        self.pending_shortcut.clear();
        self.widgets.marked_task_ids.clear();
    }

    pub(super) fn marked_task_ids_in_view(&self) -> Vec<String> {
        self.store
            .tasks
            .iter()
            .filter(|item| self.widgets.marked_task_ids.contains(&item.task.id))
            .map(|item| item.task.id.clone())
            .collect()
    }

    pub(super) fn prune_task_marks(&mut self) {
        let visible = self
            .store
            .tasks
            .iter()
            .map(|item| item.task.id.clone())
            .collect::<BTreeSet<_>>();
        self.widgets
            .marked_task_ids
            .retain(|id| visible.contains(id));
    }

    pub(super) async fn submit_add_dependency(&mut self, depends_on_task_id: String) -> Result<()> {
        match self
            .store
            .add_dependency(self.widgets.table.selected(), &depends_on_task_id)
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_info("no selected task to edit"),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.begin_add_dependency().await?;
            }
        }
        Ok(())
    }

    pub(super) async fn submit_remove_dependency(
        &mut self,
        depends_on_task_id: String,
    ) -> Result<()> {
        match self
            .store
            .remove_dependency(self.widgets.table.selected(), &depends_on_task_id)
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_info("no selected task to edit"),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.begin_remove_dependency();
            }
        }
        Ok(())
    }
}

impl App {
    pub(super) fn open_description_external_editor(&mut self, state: MultilineInputState) {
        self.needs_terminal_clear = true;
        match edit_text_externally(state.lines.join("\n"), "description.md") {
            Ok(value) => self.overlay = Some(description_overlay_from_value(value)),
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.overlay = Some(OverlayState::MultilineInput(state));
            }
        }
    }
}

fn description_overlay_from_value(value: String) -> OverlayState {
    OverlayState::multiline_input(
        OverlayRoute::EditDescription,
        EDIT_DESCRIPTION_TITLE,
        "",
        value,
    )
}

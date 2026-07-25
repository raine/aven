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
use crate::tui::task_selection::TaskSelection;

pub(crate) const EDIT_TITLE_TITLE: &str = "Edit title";
pub(crate) const EDIT_DESCRIPTION_TITLE: &str = "Edit description";
#[cfg(test)]
pub(crate) const EDIT_PROJECT_TITLE: &str = "Edit project";
#[cfg(test)]
pub(crate) const EDIT_AVAILABILITY_TITLE: &str = "Edit availability";
pub(crate) const EDIT_AVAILABILITY_PROMPT: &str =
    "Try tomorrow · in 2 weeks · next monday at 9am\nLocal dates/times · empty or now = immediate";
#[cfg(test)]
pub(crate) const EDIT_DUE_TITLE: &str = "Edit due date";
pub(crate) const EDIT_DUE_PROMPT: &str = "Try today · tomorrow · in 2 weeks · next monday\nCalendar dates only · empty or none = no due date";
pub(crate) const EDIT_LABELS_TITLE: &str = "Edit task: labels";
pub(crate) const REMOVE_DEPENDENCY_TITLE: &str = "Remove dependency";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum EditAggregate {
    #[default]
    Uniform,
    Mixed,
}

impl App {
    pub(super) async fn update_status(&mut self, status: TaskStatus) -> Result<()> {
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to edit");
            return Ok(());
        };
        let preserve_task = self.status_change_preserves_task();
        let changed_task_id = self.changed_status_target(&selection, status);
        let viewport_row = (!preserve_task)
            .then(|| self.selected_task_viewport_row())
            .flatten();
        let result = self
            .store
            .mutate_status_selection(&selection, status, preserve_task)
            .await?;
        self.apply_status_mutation_result(result, changed_task_id, viewport_row);
        Ok(())
    }

    fn status_change_preserves_task(&self) -> bool {
        self.store.view_state.view == crate::tui::store::TaskView::Columns
            || self.detail_context
            || matches!(self.overlay, Some(OverlayState::Detail { .. }))
    }

    fn changed_status_target(
        &self,
        selection: &TaskSelection,
        status: TaskStatus,
    ) -> Option<crate::ids::TaskId> {
        let selected = selection
            .targets()
            .iter()
            .find(|item| item.task.id == *selection.anchor_id() && item.task.status != status);
        selected
            .or_else(|| {
                selection
                    .targets()
                    .iter()
                    .find(|item| item.task.status != status)
            })
            .map(|item| item.task.id.clone())
    }

    fn selected_task_viewport_row(&self) -> Option<usize> {
        let selected = self.widgets.table.selected()?;
        crate::tui::ui::task_visual_row(&self.store, selected)
            .map(|row| row.saturating_sub(self.widgets.table.offset()))
    }

    fn apply_status_mutation_result(
        &mut self,
        mut result: crate::tui::store::MutationMessage,
        changed_task_id: Option<crate::ids::TaskId>,
        viewport_row: Option<usize>,
    ) {
        if let Some(task_id) = changed_task_id {
            let selection_changed = result
                .selected
                .and_then(|index| self.store.tasks.get(index))
                .is_none_or(|item| item.task.id != task_id);
            self.last_changed_task_id = Some(task_id);
            if selection_changed {
                result.message.push_str(" · g . return");
            }
        }
        self.apply_mutation_result(result);
        if let (Some(viewport_row), Some(selected)) = (viewport_row, self.widgets.table.selected())
            && let Some(visual_row) = crate::tui::ui::task_visual_row(&self.store, selected)
        {
            *self.widgets.table.offset_mut() = visual_row.saturating_sub(viewport_row);
        }
    }

    pub(super) async fn move_tasks_by_column(&mut self, delta: isize) -> Result<()> {
        if self.store.view_state.view != crate::tui::store::TaskView::Columns {
            self.set_info("column moves are available in Columns view");
            return Ok(());
        }
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to move");
            return Ok(());
        };
        let mut changes = Vec::with_capacity(selection.len());
        for item in selection.targets() {
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
            changes.push((item.task.id.clone(), status));
        }
        self.apply_column_moves(&selection, changes).await
    }

    pub(super) fn begin_move_to_column(&mut self) {
        if self.store.view_state.view != crate::tui::store::TaskView::Columns {
            self.set_info("column moves are available in Columns view");
            return;
        }
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to move");
            return;
        };
        let selected_lane = if selection.is_single() {
            crate::tui::columns::lane_index_for_status(
                &self.store.task_columns,
                selection.targets()[0].task.status,
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
                let label = if column.name.eq_ignore_ascii_case(status.as_str()) {
                    column.name.clone()
                } else {
                    format!("{} → {}", column.name, status.as_str())
                };
                Some(PickerItem {
                    label,
                    value: status.as_str().to_string(),
                    selected: selected_lane == Some(index),
                })
            })
            .collect();
        let title = if selection.is_single() {
            "Move to column".to_string()
        } else {
            format!("Move to column: {} marked tasks", selection.len())
        };
        self.edit_selection = Some(selection);
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
        let selection = self.edit_selection().clone();
        let changes = selection
            .targets()
            .iter()
            .filter_map(|item| {
                let source_lane = crate::tui::columns::lane_index_for_status(
                    &self.store.task_columns,
                    item.task.status,
                )?;
                (source_lane != target_lane).then(|| (item.task.id.clone(), target_status))
            })
            .collect::<Vec<_>>();
        if changes.is_empty() {
            self.set_info("tasks are already in that column");
            return Ok(());
        }
        self.apply_column_moves(&selection, changes).await
    }

    async fn apply_column_moves(
        &mut self,
        selection: &TaskSelection,
        changes: Vec<(crate::ids::TaskId, TaskStatus)>,
    ) -> Result<()> {
        let result = self
            .store
            .mutate_status_changes(selection, &changes)
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn set_exact_priority(
        &mut self,
        priority: crate::choices::TaskPriority,
    ) -> Result<()> {
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to edit");
            return Ok(());
        };
        let result = self
            .store
            .mutate_priority_selection(
                &selection,
                crate::tui::store::PriorityMutation::Set(priority),
            )
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn update_priority(&mut self, reverse: bool) -> Result<()> {
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to edit");
            return Ok(());
        };
        let result = self
            .store
            .mutate_priority_selection(
                &selection,
                crate::tui::store::PriorityMutation::Cycle { reverse },
            )
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn update_deleted(&mut self, deleted: bool) -> Result<()> {
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task to edit");
            return Ok(());
        };
        let result = self
            .store
            .mutate_deleted_selection(&selection, deleted)
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn submit_delete_selection(&mut self) -> Result<()> {
        let Some(selection) = self
            .edit_selection
            .clone()
            .or_else(|| self.resolve_task_selection())
        else {
            self.set_info("no selected task to edit");
            return Ok(());
        };
        let result = self
            .store
            .mutate_deleted_selection(&selection, true)
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn undo_last(&mut self) -> Result<()> {
        let in_detail =
            self.detail_context || matches!(self.overlay, Some(OverlayState::Detail { .. }));
        let selected = if in_detail {
            None
        } else {
            self.widgets.table.selected()
        };
        let focused_link = if in_detail {
            self.store
                .selected_task(self.widgets.table.selected())
                .and_then(|parent| {
                    let crate::tui::app::DetailTargetId::Task {
                        section: crate::tui::app::DetailSection::EpicChildren,
                        task_id: child_id,
                    } = self.detail_focus.as_ref()?
                    else {
                        return None;
                    };
                    parent
                        .epic_children
                        .iter()
                        .enumerate()
                        .find(|(_, child)| &child.task_id == child_id)
                        .map(|(position, child)| (parent.task.id.clone(), position, child.clone()))
                })
        } else {
            None
        };
        match self.store.undo_last(selected).await? {
            Some(result) => {
                self.apply_mutation_result(result);
                if let Some(removed) = self.removed_epic_child.take() {
                    self.widgets.table.select(
                        self.store
                            .tasks
                            .iter()
                            .position(|item| item.task.id == removed.epic_id),
                    );
                    self.detail_focus = Some(crate::tui::app::DetailTargetId::Task {
                        section: crate::tui::app::DetailSection::EpicChildren,
                        task_id: removed.child.task_id,
                    });
                } else if let Some((epic_id, original_position, child)) = focused_link
                    && self.store.tasks.iter().any(|item| {
                        item.task.id == epic_id
                            && item
                                .epic_children
                                .iter()
                                .all(|candidate| candidate.task_id != child.task_id)
                    })
                {
                    self.widgets.table.select(
                        self.store
                            .tasks
                            .iter()
                            .position(|item| item.task.id == epic_id),
                    );
                    self.detail_focus = Some(crate::tui::app::DetailTargetId::Task {
                        section: crate::tui::app::DetailSection::EpicChildren,
                        task_id: child.task_id.clone(),
                    });
                    self.removed_epic_child = Some(crate::tui::app::RemovedEpicChild {
                        epic_id,
                        child,
                        original_position,
                    });
                }
            }
            None => self.set_info("nothing to undo"),
        }
        Ok(())
    }

    fn resolve_task_selection(&self) -> Option<TaskSelection> {
        TaskSelection::resolve(
            &self.store.tasks,
            &self.widgets.marked_task_ids,
            self.widgets.table.selected(),
        )
    }

    fn capture_edit_selection(&mut self) -> bool {
        self.pending_shortcut.clear();
        self.edit_selection = self.resolve_task_selection();
        if self.edit_selection.is_none() {
            self.set_info("no selected task to edit");
            return false;
        }
        true
    }

    fn capture_single_edit_selection(&mut self, field: &str) -> bool {
        if !self.capture_edit_selection() {
            return false;
        }
        let selection = self.edit_selection.as_ref().unwrap();
        if !selection.is_single() {
            self.set_info(format!(
                "{field} requires one task · {} tasks marked",
                selection.len()
            ));
            return false;
        }
        true
    }

    fn edit_selection(&self) -> &TaskSelection {
        self.edit_selection
            .as_ref()
            .expect("edit flows capture a task selection")
    }

    fn edit_selection_items(&self) -> &[TaskListItem] {
        self.edit_selection().targets()
    }

    fn edit_selection_index(&self) -> Option<usize> {
        let task_id = self.edit_selection().single_id()?;
        self.store
            .tasks
            .iter()
            .position(|item| &item.task.id == task_id)
    }

    fn batch_edit_title(&self, field: &str) -> String {
        match self.edit_selection().len() {
            1 => format!("Edit {field}"),
            count => format!("Edit {field} · {count} marked tasks"),
        }
    }

    fn aggregate_value<T, F>(&self, value: F) -> (EditAggregate, T)
    where
        T: Clone + PartialEq + Default,
        F: Fn(&TaskListItem) -> T,
    {
        let items = self.edit_selection_items();
        let first_value = value(&items[0]);
        if items.iter().skip(1).all(|item| value(item) == first_value) {
            (EditAggregate::Uniform, first_value)
        } else {
            (EditAggregate::Mixed, T::default())
        }
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
        if !self.capture_edit_selection() {
            return;
        }
        self.footer_choice_mode = Some(FooterChoiceMode::Status);
    }

    pub(super) fn begin_edit_title(&mut self) {
        if !self.capture_single_edit_selection("title") {
            return;
        }
        let Some(index) = self.edit_selection_index() else {
            self.set_info("no selected task to edit");
            return;
        };
        let title = self.store.tasks[index].task.title.clone();
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
        if !self.capture_single_edit_selection("description") {
            return;
        }
        let Some(index) = self.edit_selection_index() else {
            self.set_info("no selected task to edit");
            return;
        };
        let description = self.store.tasks[index].task.description.clone();
        self.open_edit_description_overlay(description);
    }

    pub(super) fn begin_edit_project(&mut self) {
        if !self.capture_edit_selection() {
            return;
        }
        let (aggregate, selected) = self.aggregate_value(|item| item.task.project_key.clone());
        self.edit_aggregate = aggregate;
        let mut items = self.store.existing_project_picker_items(&selected);
        if aggregate == EditAggregate::Mixed {
            items.insert(
                0,
                PickerItem {
                    label: "Keep existing values (current: varies)".to_string(),
                    value: String::new(),
                    selected: true,
                },
            );
        }
        self.open_picker_overlay(
            OverlayRoute::EditProject,
            self.batch_edit_title("project"),
            items,
            false,
        );
    }

    pub(super) fn begin_edit_priority(&mut self) {
        if !self.capture_edit_selection() {
            return;
        }
        let (aggregate, selected) = self.aggregate_value(|item| item.task.priority.to_string());
        self.edit_aggregate = aggregate;
        if self.edit_selection().len() == 1 {
            self.footer_choice_mode = Some(FooterChoiceMode::Priority);
            return;
        }
        let mut items = self.store.priority_picker_items(&selected);
        if aggregate == EditAggregate::Mixed {
            items.insert(
                0,
                PickerItem {
                    label: "Keep existing values (current: varies)".to_string(),
                    value: String::new(),
                    selected: true,
                },
            );
        }
        self.open_picker_overlay(
            OverlayRoute::EditPriority,
            self.batch_edit_title("priority"),
            items,
            false,
        );
    }

    pub(super) fn begin_edit_availability(&mut self) {
        if !self.capture_edit_selection() {
            return;
        }
        let (aggregate, available_at) =
            self.aggregate_value(|item| item.task.available_at.clone().unwrap_or_default());
        self.edit_aggregate = aggregate;
        self.open_edit_availability_overlay(available_at);
    }

    fn open_edit_availability_overlay(&mut self, input: String) {
        let prompt = if self.edit_aggregate == EditAggregate::Mixed {
            "Current: varies\nType a date to set it on all tasks"
        } else if self.edit_selection().len() > 1 {
            "Try tomorrow · in 2 weeks · next monday at 9am"
        } else {
            EDIT_AVAILABILITY_PROMPT
        };
        self.overlay = Some(OverlayState::text_input(
            OverlayRoute::EditAvailability,
            self.batch_edit_title("availability"),
            prompt,
            input,
        ));
    }

    pub(super) fn begin_edit_due(&mut self) {
        if !self.capture_edit_selection() {
            return;
        }
        let (aggregate, due_on) =
            self.aggregate_value(|item| item.task.due_on.clone().unwrap_or_default());
        self.edit_aggregate = aggregate;
        self.open_edit_due_overlay(due_on);
    }

    fn open_edit_due_overlay(&mut self, input: String) {
        let prompt = if self.edit_aggregate == EditAggregate::Mixed {
            "Current: varies\nType a date to set it on all tasks"
        } else if self.edit_selection().len() > 1 {
            "Try today · tomorrow · in 2 weeks · next monday"
        } else {
            EDIT_DUE_PROMPT
        };
        self.overlay = Some(OverlayState::text_input(
            OverlayRoute::EditDue,
            self.batch_edit_title("due date"),
            prompt,
            input,
        ));
    }

    pub(super) fn begin_edit_labels(&mut self) {
        if !self.capture_edit_selection() {
            return;
        }
        if self.edit_selection().len() > 1 {
            self.open_edit_labels_multi();
            return;
        }
        let labels = self
            .edit_selection_items()
            .first()
            .map(|item| item.labels.clone())
            .unwrap_or_default();
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
        let status = TaskStatus::parse(&status)?;
        let selection = self.edit_selection().clone();
        let preserve_task = self.status_change_preserves_task();
        let changed_task_id = self.changed_status_target(&selection, status);
        let viewport_row = (!preserve_task)
            .then(|| self.selected_task_viewport_row())
            .flatten();
        match self
            .store
            .mutate_status_selection(&selection, status, preserve_task)
            .await
        {
            Ok(result) => self.apply_status_mutation_result(result, changed_task_id, viewport_row),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.footer_choice_mode = Some(FooterChoiceMode::Status);
            }
        }
        Ok(())
    }

    pub(super) async fn submit_edit_title(&mut self, value: String) -> Result<()> {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            self.set_warning("task title is required");
            self.open_edit_title_overlay(value);
            return Ok(());
        }
        let selection = self.edit_selection().clone();
        match self
            .store
            .mutate_text_selection(&selection, crate::tui::store::TaskTextField::Title, trimmed)
            .await
        {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.open_edit_title_overlay(value);
            }
        }
        Ok(())
    }

    pub(super) async fn submit_edit_description(&mut self, value: String) -> Result<()> {
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_text_selection(
                &selection,
                crate::tui::store::TaskTextField::Description,
                value.clone(),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.open_edit_description_overlay(value));
        Ok(())
    }

    pub(super) async fn submit_edit_project(&mut self, project: String) -> Result<()> {
        if project.is_empty() && self.edit_aggregate == EditAggregate::Mixed {
            self.set_info(format!(
                "project unchanged on {} tasks",
                self.edit_selection().len()
            ));
            return Ok(());
        }
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_project_selection(&selection, project)
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.reopen_edit_project());
        Ok(())
    }

    fn reopen_edit_project(&mut self) {
        let selection = self.edit_selection.clone();
        self.begin_edit_project();
        self.edit_selection = selection;
    }

    pub(super) async fn submit_edit_priority(&mut self, priority: String) -> Result<()> {
        if priority.is_empty() && self.edit_aggregate == EditAggregate::Mixed {
            self.set_info(format!(
                "priority unchanged on {} tasks",
                self.edit_selection().len()
            ));
            return Ok(());
        }
        let priority = crate::choices::TaskPriority::parse(&priority)?;
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_priority_selection(
                &selection,
                crate::tui::store::PriorityMutation::Set(priority),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.reopen_edit_priority());
        Ok(())
    }

    fn reopen_edit_priority(&mut self) {
        let selection = self.edit_selection.clone();
        self.begin_edit_priority();
        self.edit_selection = selection;
    }

    pub(super) async fn submit_edit_availability(&mut self, input: String) -> Result<()> {
        if self.edit_selection().len() > 1 && input.trim().is_empty() {
            if self.edit_aggregate == EditAggregate::Mixed {
                self.set_info(format!(
                    "availability unchanged on {} tasks",
                    self.edit_selection().len()
                ));
            } else {
                self.set_info("use Ctrl+D to clear availability on marked tasks");
                self.open_edit_availability_overlay(input);
            }
            return Ok(());
        }
        let available_at = if input.trim().is_empty() {
            String::new()
        } else {
            match crate::time_input::parse_available_at_input(&input) {
                Ok(value) => value,
                Err(error) => {
                    self.open_edit_availability_overlay(input);
                    self.set_warning(crate::time_input::available_at_error_message(&error));
                    return Ok(());
                }
            }
        };
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Availability,
                (!available_at.is_empty()).then_some(available_at),
                self.detail_context,
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.open_edit_availability_overlay(input));
        Ok(())
    }

    pub(super) async fn submit_edit_due(&mut self, input: String) -> Result<()> {
        if self.edit_selection().len() > 1 && input.trim().is_empty() {
            if self.edit_aggregate == EditAggregate::Mixed {
                self.set_info(format!(
                    "due date unchanged on {} tasks",
                    self.edit_selection().len()
                ));
            } else {
                self.set_info("use Ctrl+D to clear due dates on marked tasks");
                self.open_edit_due_overlay(input);
            }
            return Ok(());
        }
        let due_on = if input.trim().is_empty() {
            String::new()
        } else {
            match crate::time_input::parse_due_on_input(&input) {
                Ok(value) => value,
                Err(error) => {
                    self.open_edit_due_overlay(input);
                    self.set_warning(crate::time_input::due_on_error_message(&error));
                    return Ok(());
                }
            }
        };
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Due,
                (!due_on.is_empty()).then_some(due_on),
                self.detail_context,
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.open_edit_due_overlay(input));
        Ok(())
    }

    pub(super) async fn begin_clear_edit_value(&mut self, route: OverlayRoute) -> Result<()> {
        if self.edit_selection().len() <= 1 {
            match route {
                OverlayRoute::EditAvailability => self.submit_clear_availability().await?,
                OverlayRoute::EditDue => self.submit_clear_due().await?,
                _ => {}
            }
            return Ok(());
        }
        let (confirm_route, field) = match route {
            OverlayRoute::EditAvailability => {
                (OverlayRoute::ClearAvailabilityConfirm, "availability")
            }
            OverlayRoute::EditDue => (OverlayRoute::ClearDueConfirm, "due date"),
            _ => return Ok(()),
        };
        self.overlay = Some(OverlayState::confirm(
            confirm_route,
            format!("Clear {field}"),
            format!(
                "Clear {field} on {} marked tasks?",
                self.edit_selection().len()
            ),
        ));
        Ok(())
    }

    pub(super) async fn submit_clear_availability(&mut self) -> Result<()> {
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Availability,
                None,
                self.detail_context,
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| {
            app.open_edit_availability_overlay(String::new())
        });
        Ok(())
    }

    pub(super) async fn submit_clear_due(&mut self) -> Result<()> {
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Due,
                None,
                self.detail_context,
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.open_edit_due_overlay(String::new()));
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
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_labels_selection(&selection, labels, Vec::new())
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.begin_edit_labels());
        Ok(())
    }

    fn open_edit_labels_multi(&mut self) {
        let task_ids = self.edit_selection().ids().cloned().collect::<Vec<_>>();
        let items = self.edit_selection_items();
        let mut options = self.store.labels.iter().cloned().collect::<BTreeSet<_>>();
        for item in items {
            options.extend(item.labels.iter().cloned());
        }
        let options = options.into_iter().collect::<Vec<_>>();
        let selected = options
            .iter()
            .filter(|label| items.iter().all(|item| item.labels.contains(label)))
            .cloned()
            .collect::<Vec<_>>();
        let partial = options
            .iter()
            .filter(|label| {
                let count = items
                    .iter()
                    .filter(|item| item.labels.contains(label))
                    .count();
                count > 0 && count < items.len()
            })
            .cloned()
            .collect::<Vec<_>>();
        let count = task_ids.len();
        self.overlay = Some(OverlayState::partial_tag_combobox(
            OverlayRoute::EditLabelsMulti,
            format!("Edit labels · {count} marked tasks"),
            options,
            selected,
            partial,
        ));
    }

    pub(super) async fn submit_edit_labels_multi(
        &mut self,
        labels: Vec<String>,
        partial_labels: Vec<String>,
    ) -> Result<()> {
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
        let selection = self.edit_selection().clone();
        let result = self
            .store
            .mutate_labels_selection(&selection, labels, partial_labels)
            .await
            .map(Some);
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

    pub(super) fn marked_task_ids_in_view(&self) -> Vec<crate::ids::TaskId> {
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

    pub(super) async fn submit_add_dependency(
        &mut self,
        depends_on_task_id: crate::ids::TaskId,
    ) -> Result<()> {
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
        depends_on_task_id: crate::ids::TaskId,
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

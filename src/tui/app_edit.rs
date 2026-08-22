use std::collections::BTreeSet;

use anyhow::Result;

use crate::choices::TaskStatus;
use crate::query::TaskListItem;
use crate::tui::app::{App, FooterChoiceMode, FooterChoiceState};
use crate::tui::overlay::{
    ConfirmIntent, MultilineInputState, MultilineIntent, OverlayState, PickerIntent, PickerItem,
    SearchIntent, SearchState, TagComboboxIntent, TextIntent,
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
pub(crate) const REMOVE_RELATED_TITLE: &str = "Remove related task";

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
            || self.detail.is_active()
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
        let selected = self.list.selected_task()?;
        crate::tui::ui::task_visual_row(&self.store, selected)
            .map(|row| row.saturating_sub(self.list.task_offset()))
    }

    fn record_changed_task_result(
        &mut self,
        result: &mut crate::tui::store::MutationMessage,
        changed_task_id: Option<crate::ids::TaskId>,
    ) {
        let Some(task_id) = changed_task_id else {
            return;
        };
        let selection_changed = result
            .selected
            .and_then(|index| self.store.tasks.get(index))
            .is_none_or(|item| item.task.id != task_id);
        self.list.record_changed_task(task_id);
        if selection_changed {
            result.message.push_str(" · g . return");
        }
    }

    fn apply_status_mutation_result(
        &mut self,
        mut result: crate::tui::store::MutationMessage,
        changed_task_id: Option<crate::ids::TaskId>,
        viewport_row: Option<usize>,
    ) {
        self.record_changed_task_result(&mut result, changed_task_id);
        self.apply_mutation_result(result);
        if let (Some(viewport_row), Some(selected)) = (viewport_row, self.list.selected_task())
            && let Some(visual_row) = crate::tui::ui::task_visual_row(&self.store, selected)
        {
            self.list
                .set_task_offset(visual_row.saturating_sub(viewport_row));
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
        self.open_move_to_column_picker(selection);
    }

    pub(super) fn open_move_to_column_picker(&mut self, selection: TaskSelection) {
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
        self.open_picker_overlay(
            PickerIntent::MoveToColumn { selection },
            title,
            items,
            false,
        );
    }

    pub(super) async fn move_tasks_to_column(
        &mut self,
        selection: TaskSelection,
        status: String,
    ) -> Result<()> {
        let target_status = TaskStatus::parse(&status)?;
        let Some(target_lane) =
            crate::tui::columns::lane_index_for_status(&self.store.task_columns, target_status)
        else {
            self.set_warning("column is unavailable");
            return Ok(());
        };
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
        let retry_selection = selection.clone();
        if let Err(error) = self.apply_column_moves(&selection, changes).await {
            let committed = crate::tui::store::mutation_committed(&error);
            self.set_error(format!("{error:#}"));
            if !committed {
                self.open_move_to_column_picker(retry_selection);
            }
        }
        Ok(())
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
        let preserve_task = self.detail.is_active();
        let result = self
            .store
            .mutate_deleted_selection(&selection, deleted, preserve_task)
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn submit_delete_selection(&mut self, selection: TaskSelection) -> Result<()> {
        let preserve_task = self.detail.is_active();
        let result = self
            .store
            .mutate_deleted_selection(&selection, true, preserve_task)
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn submit_delete_focused_task(
        &mut self,
        selection: TaskSelection,
    ) -> Result<()> {
        let result = self
            .store
            .mutate_deleted_selection(&selection, true, false)
            .await?;
        self.apply_mutation_result(result);
        Ok(())
    }

    pub(super) async fn undo_last(&mut self) -> Result<()> {
        let in_detail = self.detail.is_active();
        let detail_anchor_id = in_detail
            .then(|| {
                self.store
                    .selected_task(self.list.selected_task())
                    .map(|item| item.task.id.clone())
            })
            .flatten();
        let selected = if in_detail {
            None
        } else {
            self.list.selected_task()
        };
        let focused_link = if in_detail {
            self.store
                .selected_task(self.list.selected_task())
                .and_then(|parent| {
                    let crate::tui::app::DetailTargetId::Task {
                        section: crate::tui::app::DetailSection::EpicChildren,
                        task_id: child_id,
                    } = self.detail.state()?.focused_target()?
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
            Some(mut result) => {
                if let Some(anchor_id) = detail_anchor_id {
                    result.selected = self.store.refresh(Some(&anchor_id)).await?;
                }
                self.store.new_undo_entry_id = None;
                self.apply_mutation_result(result);
                let removed = self
                    .detail
                    .state_mut()
                    .and_then(|detail| detail.take_removed_epic_child());
                if let Some(removed) = removed {
                    self.list.select_task(
                        self.store
                            .tasks
                            .iter()
                            .position(|item| item.task.id == removed.epic_id),
                    );
                    if let Some(detail) = self.detail.state_mut() {
                        detail.set_focused_target(Some(crate::tui::app::DetailTargetId::Task {
                            section: crate::tui::app::DetailSection::EpicChildren,
                            task_id: removed.child.task_id,
                        }));
                    }
                } else if let Some((epic_id, original_position, child)) = focused_link
                    && self.store.tasks.iter().any(|item| {
                        item.task.id == epic_id
                            && item
                                .epic_children
                                .iter()
                                .all(|candidate| candidate.task_id != child.task_id)
                    })
                {
                    self.list.select_task(
                        self.store
                            .tasks
                            .iter()
                            .position(|item| item.task.id == epic_id),
                    );
                    if let Some(detail) = self.detail.state_mut() {
                        detail.set_focused_target(Some(crate::tui::app::DetailTargetId::Task {
                            section: crate::tui::app::DetailSection::EpicChildren,
                            task_id: child.task_id.clone(),
                        }));
                        detail.set_removed_epic_child(Some(crate::tui::app::RemovedEpicChild {
                            epic_id,
                            child,
                            original_position,
                        }));
                    }
                }
            }
            None => self.set_info("nothing to undo"),
        }
        Ok(())
    }

    pub(super) fn selected_command_task(&self) -> Option<TaskListItem> {
        self.detail_command_selection
            .as_ref()
            .and_then(|selection| selection.targets().first().cloned())
            .or_else(|| self.store.selected_task(self.list.selected_task()).cloned())
    }

    pub(super) fn resolve_task_selection(&self) -> Option<TaskSelection> {
        if let Some(selection) = &self.detail_command_selection {
            return Some(selection.clone());
        }
        if self.detail.is_active() {
            TaskSelection::resolve_single(&self.store.tasks, self.list.selected_task())
        } else {
            TaskSelection::resolve(
                &self.store.tasks,
                self.list.marked_task_ids(),
                self.list.selected_task(),
            )
        }
    }

    fn capture_edit_selection(&mut self) -> Option<TaskSelection> {
        self.pending_shortcut.clear();
        let selection = self.resolve_task_selection();
        if selection.is_none() {
            self.set_info("no selected task to edit");
        }
        selection
    }

    fn capture_single_edit_selection(&mut self, field: &str) -> Option<TaskSelection> {
        let selection = self.capture_edit_selection()?;
        if !selection.is_single() {
            self.set_info(format!(
                "{field} requires one task · {} tasks marked",
                selection.len()
            ));
            return None;
        }
        Some(selection)
    }

    fn selection_index(&self, selection: &TaskSelection) -> Option<usize> {
        let task_id = selection.single_id()?;
        self.store
            .tasks
            .iter()
            .position(|item| &item.task.id == task_id)
    }

    fn batch_edit_title(selection: &TaskSelection, field: &str) -> String {
        match selection.len() {
            1 => format!("Edit {field}"),
            count => format!("Edit {field} · {count} marked tasks"),
        }
    }

    fn aggregate_value<T, F>(selection: &TaskSelection, value: F) -> (EditAggregate, T)
    where
        T: Clone + PartialEq + Default,
        F: Fn(&TaskListItem) -> T,
    {
        let items = selection.targets();
        let first_value = value(&items[0]);
        if items.iter().skip(1).all(|item| value(item) == first_value) {
            (EditAggregate::Uniform, first_value)
        } else {
            (EditAggregate::Mixed, T::default())
        }
    }

    fn apply_edit_mutation<F>(
        &mut self,
        result: Result<Option<crate::tui::store::MutationMessage>>,
        on_error: F,
    ) where
        F: FnOnce(&mut Self),
    {
        self.apply_changed_edit_mutation(result, None, on_error);
    }

    fn apply_changed_edit_mutation<F>(
        &mut self,
        result: Result<Option<crate::tui::store::MutationMessage>>,
        changed_task_id: Option<crate::ids::TaskId>,
        on_error: F,
    ) where
        F: FnOnce(&mut Self),
    {
        match result {
            Ok(Some(mut result)) => {
                self.record_changed_task_result(&mut result, changed_task_id);
                self.apply_mutation_result(result);
            }
            Ok(None) => self.set_info("no selected task to edit"),
            Err(error) => {
                let committed = crate::tui::store::mutation_committed(&error);
                self.set_error(format!("{error:#}"));
                if !committed {
                    on_error(self);
                }
            }
        }
    }

    pub(super) fn begin_status_picker(&mut self) {
        let Some(selection) = self.capture_edit_selection() else {
            return;
        };
        self.footer_choice = Some(FooterChoiceState {
            mode: FooterChoiceMode::Status,
            selection,
        });
    }

    pub(super) fn begin_edit_title(&mut self) {
        let Some(selection) = self.capture_single_edit_selection("title") else {
            return;
        };
        let title = selection.targets()[0].task.title.clone();
        self.open_edit_title_overlay(selection, title);
    }

    fn open_edit_title_overlay(&mut self, selection: TaskSelection, input: String) {
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(0);
        }
        self.overlay = Some(OverlayState::text_input(
            TextIntent::EditTitle { selection },
            EDIT_TITLE_TITLE,
            "",
            input,
        ));
    }

    fn open_edit_description_overlay(&mut self, selection: TaskSelection, value: String) {
        self.overlay = Some(OverlayState::multiline_input(
            MultilineIntent::EditDescription { selection },
            EDIT_DESCRIPTION_TITLE,
            "",
            value,
        ));
    }

    pub(super) fn begin_edit_description(&mut self) {
        let Some(selection) = self.capture_single_edit_selection("description") else {
            return;
        };
        let description = selection.targets()[0].task.description.clone();
        self.open_edit_description_overlay(selection, description);
    }

    pub(super) fn begin_edit_project(&mut self) {
        let Some(selection) = self.capture_edit_selection() else {
            return;
        };
        self.open_edit_project_picker(selection);
    }

    pub(super) fn open_edit_project_picker(&mut self, selection: TaskSelection) {
        let (aggregate, selected) =
            Self::aggregate_value(&selection, |item| item.task.project_key.clone());
        let mut items = self.store.edit_project_picker_items(&selected);
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
        let title = Self::batch_edit_title(&selection, "project");
        self.open_picker_overlay(
            PickerIntent::EditProject {
                selection,
                mixed: aggregate == EditAggregate::Mixed,
            },
            title,
            items,
            false,
        );
    }

    pub(super) fn begin_edit_priority(&mut self) {
        let Some(selection) = self.capture_edit_selection() else {
            return;
        };
        if selection.len() == 1 {
            self.footer_choice = Some(FooterChoiceState {
                mode: FooterChoiceMode::Priority,
                selection,
            });
            return;
        }
        self.open_edit_priority_picker_for_selection(selection);
    }

    pub(super) fn open_edit_priority_picker_for_selection(&mut self, selection: TaskSelection) {
        let (aggregate, selected) =
            Self::aggregate_value(&selection, |item| item.task.priority.to_string());
        self.open_edit_priority_picker(selection, aggregate, selected);
    }

    fn open_edit_priority_picker(
        &mut self,
        selection: TaskSelection,
        aggregate: EditAggregate,
        selected: String,
    ) {
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
        let title = Self::batch_edit_title(&selection, "priority");
        self.open_picker_overlay(
            PickerIntent::EditPriority {
                selection,
                mixed: aggregate == EditAggregate::Mixed,
            },
            title,
            items,
            false,
        );
    }

    pub(super) fn begin_edit_epic(&mut self) {
        let Some(selection) = self.capture_edit_selection() else {
            return;
        };
        self.open_edit_epic_picker(selection);
    }

    pub(super) fn open_edit_epic_picker(&mut self, selection: TaskSelection) {
        let (aggregate, selected) = Self::aggregate_value(&selection, |item| item.task.is_epic);
        let mut items = crate::tui::store::epic_picker_items(
            (aggregate == EditAggregate::Uniform).then_some(selected),
        );
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
        let title = Self::batch_edit_title(&selection, "epic container");
        self.open_picker_overlay(
            PickerIntent::EditEpic {
                selection,
                mixed: aggregate == EditAggregate::Mixed,
            },
            title,
            items,
            false,
        );
    }

    pub(super) fn begin_edit_availability(&mut self) {
        let Some(selection) = self.capture_edit_selection() else {
            return;
        };
        let (aggregate, available_at) = Self::aggregate_value(&selection, |item| {
            item.task.available_at.clone().unwrap_or_default()
        });
        self.open_edit_availability_overlay(selection, aggregate, available_at);
    }

    fn open_edit_availability_overlay(
        &mut self,
        selection: TaskSelection,
        aggregate: EditAggregate,
        input: String,
    ) {
        let prompt = if aggregate == EditAggregate::Mixed {
            "Current: varies\nType a date to set it on all tasks"
        } else if selection.len() > 1 {
            "Try tomorrow · in 2 weeks · next monday at 9am"
        } else {
            EDIT_AVAILABILITY_PROMPT
        };
        let title = Self::batch_edit_title(&selection, "availability");
        self.overlay = Some(OverlayState::text_input(
            TextIntent::EditAvailability {
                selection,
                mixed: aggregate == EditAggregate::Mixed,
            },
            title,
            prompt,
            input,
        ));
    }

    pub(super) fn begin_edit_due(&mut self) {
        let Some(selection) = self.capture_edit_selection() else {
            return;
        };
        let (aggregate, due_on) = Self::aggregate_value(&selection, |item| {
            item.task.due_on.clone().unwrap_or_default()
        });
        self.open_edit_due_overlay(selection, aggregate, due_on);
    }

    fn open_edit_due_overlay(
        &mut self,
        selection: TaskSelection,
        aggregate: EditAggregate,
        input: String,
    ) {
        let prompt = if aggregate == EditAggregate::Mixed {
            "Current: varies\nType a date to set it on all tasks"
        } else if selection.len() > 1 {
            "Try today · tomorrow · in 2 weeks · next monday"
        } else {
            EDIT_DUE_PROMPT
        };
        let title = Self::batch_edit_title(&selection, "due date");
        self.overlay = Some(OverlayState::text_input(
            TextIntent::EditDue {
                selection,
                mixed: aggregate == EditAggregate::Mixed,
            },
            title,
            prompt,
            input,
        ));
    }

    pub(super) fn begin_edit_labels(&mut self) {
        let Some(selection) = self.capture_edit_selection() else {
            return;
        };
        if selection.len() > 1 {
            self.open_edit_labels_multi(selection);
            return;
        }
        let labels = selection
            .targets()
            .first()
            .map(|item| item.labels.clone())
            .unwrap_or_default();
        self.overlay = Some(OverlayState::tag_combobox(
            TagComboboxIntent::EditLabels { selection },
            EDIT_LABELS_TITLE,
            self.store.labels.clone(),
            labels,
        ));
    }

    pub(super) async fn begin_add_dependency(&mut self) -> Result<()> {
        let Some(selection) = self.capture_single_edit_selection("dependency") else {
            return Ok(());
        };
        let display_ref = selection.targets()[0].display_ref.clone();
        self.clear_live_search_preview();
        self.overlay = Some(OverlayState::Search(SearchState::for_intent(
            SearchIntent::AddDependency {
                selection,
                display_ref,
            },
        )));
        Ok(())
    }

    pub(super) async fn begin_add_related(&mut self) -> Result<()> {
        let Some(selection) = self.capture_single_edit_selection("related link") else {
            return Ok(());
        };
        let display_ref = selection.targets()[0].display_ref.clone();
        self.clear_live_search_preview();
        self.overlay = Some(OverlayState::Search(SearchState::for_intent(
            SearchIntent::AddRelated {
                selection,
                display_ref,
            },
        )));
        Ok(())
    }

    pub(super) fn begin_remove_related(&mut self) {
        let Some(selection) = self.capture_single_edit_selection("related link") else {
            return;
        };
        let Some(index) = self.selection_index(&selection) else {
            self.set_warning("task is unavailable");
            return;
        };
        let items = self.store.selected_related_picker_items(Some(index));
        self.open_picker_overlay(
            PickerIntent::RemoveRelated { selection },
            REMOVE_RELATED_TITLE,
            items,
            false,
        );
    }

    pub(super) fn confirm_remove_related(
        &mut self,
        selection: TaskSelection,
        related_task_id: crate::ids::TaskId,
    ) {
        let Some(related) = selection.targets()[0]
            .related
            .iter()
            .find(|related| related.task_id == related_task_id)
        else {
            self.set_warning("related task is unavailable");
            self.begin_remove_related();
            return;
        };
        let prompt = format!(
            "Unlink {} {} from this task?",
            related.display_ref, related.title
        );
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::UnlinkRelated {
                selection,
                related_task_id,
            },
            "Unlink relationship",
            prompt,
        ));
    }

    pub(super) fn begin_remove_dependency(&mut self) {
        let Some(selection) = self.capture_single_edit_selection("dependency") else {
            return;
        };
        self.open_remove_dependency_picker(selection);
    }

    pub(super) fn open_remove_dependency_picker(&mut self, selection: TaskSelection) {
        let Some(index) = self.selection_index(&selection) else {
            self.set_warning("task is unavailable");
            return;
        };
        let items = self.store.selected_dependency_picker_items(Some(index));
        self.open_picker_overlay(
            PickerIntent::RemoveDependency { selection },
            REMOVE_DEPENDENCY_TITLE,
            items,
            false,
        );
    }

    pub(super) async fn submit_edit_status(
        &mut self,
        selection: TaskSelection,
        status: String,
    ) -> Result<()> {
        let status = TaskStatus::parse(&status)?;
        let relationship_anchor = selection
            .single_id()
            .filter(|task_id| *task_id != selection.anchor_id())
            .map(|_| selection.anchor_id().clone());
        let preserve_task = self.status_change_preserves_task() && relationship_anchor.is_none();
        let changed_task_id = self.changed_status_target(&selection, status);
        let viewport_row = (!preserve_task)
            .then(|| self.selected_task_viewport_row())
            .flatten();
        match self
            .store
            .mutate_status_selection(&selection, status, preserve_task)
            .await
        {
            Ok(mut result) => {
                if let Some(anchor_id) = relationship_anchor {
                    result.selected = self.store.refresh(Some(&anchor_id)).await?;
                }
                self.apply_status_mutation_result(result, changed_task_id, viewport_row);
            }
            Err(error) => {
                let committed = crate::tui::store::mutation_committed(&error);
                self.set_error(format!("{error:#}"));
                if !committed {
                    self.footer_choice = Some(FooterChoiceState {
                        mode: FooterChoiceMode::Status,
                        selection,
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) async fn submit_edit_title(
        &mut self,
        selection: TaskSelection,
        value: String,
    ) -> Result<()> {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            self.set_warning("task title is required");
            self.open_edit_title_overlay(selection, value);
            return Ok(());
        }
        match self
            .store
            .mutate_text_selection(&selection, crate::tui::store::TaskTextField::Title, trimmed)
            .await
        {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => {
                let committed = crate::tui::store::mutation_committed(&error);
                self.set_error(format!("{error:#}"));
                if !committed {
                    self.open_edit_title_overlay(selection, value);
                }
            }
        }
        Ok(())
    }

    pub(super) async fn submit_edit_description(
        &mut self,
        selection: TaskSelection,
        value: String,
    ) -> Result<()> {
        let result = self
            .store
            .mutate_text_selection(
                &selection,
                crate::tui::store::TaskTextField::Description,
                value.clone(),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| {
            app.open_edit_description_overlay(selection, value)
        });
        Ok(())
    }

    fn changed_project_target(
        selection: &TaskSelection,
        project: &str,
    ) -> Option<crate::ids::TaskId> {
        let changed = selection.targets().iter().find(|item| {
            item.task.id == *selection.anchor_id() && item.task.project_key != project
        });
        changed
            .or_else(|| {
                selection
                    .targets()
                    .iter()
                    .find(|item| item.task.project_key != project)
            })
            .map(|item| item.task.id.clone())
    }

    pub(super) async fn submit_edit_project(
        &mut self,
        selection: TaskSelection,
        mixed: bool,
        project: String,
    ) -> Result<()> {
        if project.is_empty() && mixed {
            self.set_info(format!("project unchanged on {} tasks", selection.len()));
            return Ok(());
        }
        let changed_task_id = Self::changed_project_target(&selection, &project);
        let result = self
            .store
            .mutate_project_selection(&selection, project)
            .await
            .map(Some);
        self.apply_changed_edit_mutation(result, changed_task_id, |app| {
            app.open_edit_project_picker(selection)
        });
        Ok(())
    }

    pub(super) async fn submit_edit_priority(
        &mut self,
        selection: TaskSelection,
        mixed: bool,
        priority: String,
    ) -> Result<()> {
        if priority.is_empty() && mixed {
            self.set_info(format!("priority unchanged on {} tasks", selection.len()));
            return Ok(());
        }
        let priority = crate::choices::TaskPriority::parse(&priority)?;
        let result = self
            .store
            .mutate_priority_selection(
                &selection,
                crate::tui::store::PriorityMutation::Set(priority),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| {
            let (aggregate, selected) =
                Self::aggregate_value(&selection, |item| item.task.priority.to_string());
            app.open_edit_priority_picker(selection, aggregate, selected);
        });
        Ok(())
    }

    pub(super) async fn submit_edit_availability(
        &mut self,
        selection: TaskSelection,
        mixed: bool,
        input: String,
    ) -> Result<()> {
        if selection.len() > 1 && input.trim().is_empty() {
            if mixed {
                self.set_info(format!(
                    "availability unchanged on {} tasks",
                    selection.len()
                ));
            } else {
                self.set_info("use Ctrl+D to clear availability on marked tasks");
                self.open_edit_availability_overlay(selection, EditAggregate::Uniform, input);
            }
            return Ok(());
        }
        let available_at = if input.trim().is_empty() {
            String::new()
        } else {
            match crate::time_input::parse_available_at_input(&input) {
                Ok(value) => value,
                Err(error) => {
                    self.open_edit_availability_overlay(
                        selection,
                        if mixed {
                            EditAggregate::Mixed
                        } else {
                            EditAggregate::Uniform
                        },
                        input,
                    );
                    self.set_warning(crate::time_input::available_at_error_message(&error));
                    return Ok(());
                }
            }
        };
        let retry_selection = selection.clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Availability,
                (!available_at.is_empty()).then_some(available_at),
                self.detail.is_active(),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| {
            app.open_edit_availability_overlay(
                retry_selection,
                if mixed {
                    EditAggregate::Mixed
                } else {
                    EditAggregate::Uniform
                },
                input,
            )
        });
        Ok(())
    }

    pub(super) async fn submit_edit_due(
        &mut self,
        selection: TaskSelection,
        mixed: bool,
        input: String,
    ) -> Result<()> {
        if selection.len() > 1 && input.trim().is_empty() {
            if mixed {
                self.set_info(format!("due date unchanged on {} tasks", selection.len()));
            } else {
                self.set_info("use Ctrl+D to clear due dates on marked tasks");
                self.open_edit_due_overlay(selection, EditAggregate::Uniform, input);
            }
            return Ok(());
        }
        let due_on = if input.trim().is_empty() {
            String::new()
        } else {
            match crate::time_input::parse_due_on_input(&input) {
                Ok(value) => value,
                Err(error) => {
                    self.open_edit_due_overlay(
                        selection,
                        if mixed {
                            EditAggregate::Mixed
                        } else {
                            EditAggregate::Uniform
                        },
                        input,
                    );
                    self.set_warning(crate::time_input::due_on_error_message(&error));
                    return Ok(());
                }
            }
        };
        let retry_selection = selection.clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Due,
                (!due_on.is_empty()).then_some(due_on),
                self.detail.is_active(),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| {
            app.open_edit_due_overlay(
                retry_selection,
                if mixed {
                    EditAggregate::Mixed
                } else {
                    EditAggregate::Uniform
                },
                input,
            )
        });
        Ok(())
    }

    pub(super) async fn begin_clear_edit_value(&mut self, intent: TextIntent) -> Result<()> {
        let (selection, confirm_intent, field) = match intent {
            TextIntent::EditAvailability { selection, .. } => (
                selection.clone(),
                ConfirmIntent::ClearAvailability { selection },
                "availability",
            ),
            TextIntent::EditDue { selection, .. } => (
                selection.clone(),
                ConfirmIntent::ClearDue { selection },
                "due date",
            ),
            _ => return Ok(()),
        };
        if selection.len() <= 1 {
            match confirm_intent {
                ConfirmIntent::ClearAvailability { .. } => {
                    self.submit_clear_availability(selection).await?
                }
                ConfirmIntent::ClearDue { .. } => self.submit_clear_due(selection).await?,
                _ => unreachable!(),
            }
            return Ok(());
        }
        self.overlay = Some(OverlayState::confirm(
            confirm_intent,
            format!("Clear {field}"),
            format!("Clear {field} on {} marked tasks?", selection.len()),
        ));
        Ok(())
    }

    pub(super) async fn submit_clear_availability(
        &mut self,
        selection: TaskSelection,
    ) -> Result<()> {
        let retry_selection = selection.clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Availability,
                None,
                self.detail.is_active(),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| {
            let (aggregate, value) = Self::aggregate_value(&retry_selection, |item| {
                item.task.available_at.clone().unwrap_or_default()
            });
            app.open_edit_availability_overlay(retry_selection, aggregate, value)
        });
        Ok(())
    }

    pub(super) async fn submit_clear_due(&mut self, selection: TaskSelection) -> Result<()> {
        let retry_selection = selection.clone();
        let result = self
            .store
            .mutate_date_selection(
                &selection,
                crate::tui::store::TaskDateField::Due,
                None,
                self.detail.is_active(),
            )
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| {
            let (aggregate, value) = Self::aggregate_value(&retry_selection, |item| {
                item.task.due_on.clone().unwrap_or_default()
            });
            app.open_edit_due_overlay(retry_selection, aggregate, value)
        });
        Ok(())
    }

    pub(super) async fn submit_edit_epic(
        &mut self,
        selection: TaskSelection,
        mixed: bool,
        value: String,
    ) -> Result<()> {
        if value.is_empty() && mixed {
            self.set_info(format!(
                "epic container state unchanged on {} tasks",
                selection.len()
            ));
            return Ok(());
        }
        let is_epic = match value.as_str() {
            "true" => true,
            "false" => false,
            _ => {
                self.set_warning("choose whether this task is an epic container");
                self.open_edit_epic_picker(selection);
                return Ok(());
            }
        };
        if is_epic
            && selection
                .targets()
                .iter()
                .any(|item| item.epic_parent.is_some())
        {
            self.set_warning("remove a task from its parent epic before turning its container on");
            self.open_edit_epic_picker(selection);
            return Ok(());
        }
        match self.store.mutate_epic_selection(&selection, is_epic).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => {
                let committed = crate::tui::store::mutation_committed(&error);
                let message = format!("{error:#}");
                if message.contains("epic-has-children") {
                    self.set_warning("remove all epic children before turning the container off");
                } else {
                    self.set_error(message);
                }
                if !committed {
                    self.open_edit_epic_picker(selection);
                }
            }
        }
        Ok(())
    }

    pub(super) async fn submit_edit_labels(
        &mut self,
        selection: TaskSelection,
        labels: Vec<String>,
    ) -> Result<()> {
        let retry_selection = selection.clone();
        let result = self
            .store
            .mutate_labels_selection(&selection, labels, Vec::new())
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.open_edit_labels(retry_selection));
        Ok(())
    }

    fn open_edit_labels(&mut self, selection: TaskSelection) {
        if selection.len() > 1 {
            self.open_edit_labels_multi(selection);
            return;
        }
        let labels = selection
            .targets()
            .first()
            .map(|item| item.labels.clone())
            .unwrap_or_default();
        self.overlay = Some(OverlayState::tag_combobox(
            TagComboboxIntent::EditLabels { selection },
            EDIT_LABELS_TITLE,
            self.store.labels.clone(),
            labels,
        ));
    }

    fn open_edit_labels_multi(&mut self, selection: TaskSelection) {
        let items = selection.targets();
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
        let count = selection.len();
        self.overlay = Some(OverlayState::partial_tag_combobox(
            TagComboboxIntent::EditLabelsMulti { selection },
            format!("Edit labels · {count} marked tasks"),
            options,
            selected,
            partial,
        ));
    }

    pub(super) async fn submit_edit_labels_multi(
        &mut self,
        selection: TaskSelection,
        labels: Vec<String>,
        partial_labels: Vec<String>,
    ) -> Result<()> {
        let retry_selection = selection.clone();
        let result = self
            .store
            .mutate_labels_selection(&selection, labels, partial_labels)
            .await
            .map(Some);
        self.apply_edit_mutation(result, |app| app.open_edit_labels_multi(retry_selection));
        Ok(())
    }

    pub(super) fn toggle_mark_selected(&mut self) {
        self.pending_shortcut.clear();
        let Some(index) = self.list.selected_task() else {
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
        self.list.toggle_mark(id);
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
        if self.list.all_marked(&visible) {
            self.list.retain_marks(|id| !visible.contains(id));
        } else {
            self.list.mark_all(visible);
        }
    }

    pub(super) fn clear_marks(&mut self) {
        self.pending_shortcut.clear();
        self.list.clear_marks();
    }

    pub(super) fn bulk_scope_marked_task_count(&self) -> usize {
        if self.detail_command_selection.is_some() || self.detail.is_active() {
            return 0;
        }
        self.store
            .tasks
            .iter()
            .filter(|item| self.list.marked_task_ids().contains(&item.task.id))
            .count()
    }

    pub(super) fn marked_task_ids_in_view(&self) -> Vec<crate::ids::TaskId> {
        self.store
            .tasks
            .iter()
            .filter(|item| self.list.marked_task_ids().contains(&item.task.id))
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
        self.list.retain_marks(|id| visible.contains(id));
    }

    pub(super) async fn submit_remove_related(
        &mut self,
        selection: TaskSelection,
        related_task_id: crate::ids::TaskId,
    ) -> Result<()> {
        match self
            .store
            .remove_related_from_selection(&selection, &related_task_id)
            .await
        {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => self.set_error(format!("{error:#}")),
        }
        Ok(())
    }

    pub(super) async fn submit_remove_dependency(
        &mut self,
        selection: TaskSelection,
        depends_on_task_id: crate::ids::TaskId,
    ) -> Result<()> {
        let relationship_anchor = selection
            .single_id()
            .filter(|task_id| *task_id != selection.anchor_id())
            .map(|_| selection.anchor_id().clone());
        match self
            .store
            .remove_dependency_from_selection(&selection, &depends_on_task_id)
            .await
        {
            Ok(mut result) => {
                if let Some(anchor_id) = relationship_anchor {
                    result.selected = self.store.refresh(Some(&anchor_id)).await?;
                }
                self.apply_mutation_result(result);
            }
            Err(error) => {
                let committed = crate::tui::store::mutation_committed(&error);
                self.set_error(format!("{error:#}"));
                if !committed {
                    self.open_remove_dependency_picker(selection);
                }
            }
        }
        Ok(())
    }
}

impl App {
    pub(super) fn open_description_external_editor(&mut self, state: MultilineInputState) {
        self.prepare_terminal_transition();
        let intent = state.intent.clone();
        let baseline = state.baseline_value();
        match edit_text_externally(
            state.lines.join("\n"),
            "description.md",
            self.terminal_mouse_capture,
        ) {
            Ok(value) => {
                self.overlay = Some(description_overlay_from_value(intent, value, baseline))
            }
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.overlay = Some(OverlayState::MultilineInput(state));
            }
        }
    }
}

fn description_overlay_from_value(
    intent: MultilineIntent,
    value: String,
    baseline: String,
) -> OverlayState {
    OverlayState::multiline_input_with_baseline(intent, EDIT_DESCRIPTION_TITLE, "", value, baseline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_description_value_retains_initial_baseline() {
        let intent = MultilineIntent::EditDescription {
            selection: TaskSelection::resolve(
                &[crate::tui::test_support::task_list_item("Task")],
                &std::collections::BTreeSet::new(),
                Some(0),
            )
            .unwrap(),
        };
        let OverlayState::MultilineInput(state) = description_overlay_from_value(
            intent,
            "edited description".to_string(),
            "initial description".to_string(),
        ) else {
            panic!("expected multiline description editor");
        };

        assert!(state.is_dirty());
        assert_eq!(state.baseline_value(), "initial description");
    }
}

use anyhow::Result;

use crate::operations::TaskDraft;
use crate::tui::app::App;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{
    AddTaskMode, AddTaskState, ConfirmSubmitRoute, MultilineSubmitRoute, OverlayRoute,
    OverlayState, OverlaySubmit, OverlaySubmitKind, PickerSubmitRoute, TextSubmitRoute,
};

#[cfg(test)]
pub(crate) fn handles_submit_kind(route: OverlayRoute, kind: OverlaySubmitKind) -> bool {
    match kind {
        OverlaySubmitKind::Text => route.text_submit_route().is_some(),
        OverlaySubmitKind::Multiline => route.multiline_submit_route().is_some(),
        OverlaySubmitKind::Picker => route.picker_submit_route().is_some(),
        OverlaySubmitKind::Confirm => route.confirm_submit_route().is_some(),
    }
}

impl App {
    pub(super) async fn handle_overlay_submit(&mut self, submit: OverlaySubmit) -> Result<()> {
        match submit {
            OverlaySubmit::AddTask(state) => {
                self.handle_add_task_submit(*state).await?;
            }
            OverlaySubmit::Picker { route, values } => {
                self.handle_picker_submit(route, values).await?;
            }
            OverlaySubmit::HeaderMenu { action } => {
                self.submit_header_menu(action).await?;
            }
            OverlaySubmit::Order { order } => {
                self.submit_order_menu(order).await?;
            }
            OverlaySubmit::Text { route, value } => {
                self.handle_text_submit(route, value).await?;
            }
            OverlaySubmit::Multiline { route, value } => {
                self.handle_multiline_submit(route, value).await?;
            }
            OverlaySubmit::Confirm { route } => {
                self.handle_confirm_submit(route).await?;
            }
        }
        Ok(())
    }

    async fn handle_add_task_submit(&mut self, mut state: AddTaskState) -> Result<()> {
        let title = state.title.text.trim();
        if title.is_empty() {
            state.focus = AddTaskStep::Title;
            state.mode = AddTaskMode::Compose;
            state.title_error = true;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning("task title is required");
            return Ok(());
        }

        for label in state.labels.clone() {
            let label = crate::labels::normalize_label(&label);
            if !self.store.labels.contains(&label)
                && let Err(error) = self.store.create_label(label).await
            {
                state.mode = AddTaskMode::Compose;
                self.overlay = Some(OverlayState::AddTask(Box::new(state)));
                return Err(error);
            }
        }

        let available_at = if state.available_at.text.trim().is_empty() {
            None
        } else {
            match crate::time_input::parse_available_at_input(&state.available_at.text) {
                Ok(value) => Some(value),
                Err(error) => {
                    state.focus = AddTaskStep::AvailableAt;
                    state.mode = AddTaskMode::Compose;
                    self.overlay = Some(OverlayState::AddTask(Box::new(state)));
                    self.set_warning(crate::time_input::available_at_error_message(&error));
                    return Ok(());
                }
            }
        };
        let due_on = if state.due_on.text.trim().is_empty() {
            None
        } else {
            match crate::time_input::parse_due_on_input(&state.due_on.text) {
                Ok(value) => Some(value),
                Err(error) => {
                    state.focus = AddTaskStep::Due;
                    state.mode = AddTaskMode::Compose;
                    self.overlay = Some(OverlayState::AddTask(Box::new(state)));
                    self.set_warning(crate::time_input::due_on_error_message(&error));
                    return Ok(());
                }
            }
        };
        let draft = TaskDraft {
            title: title.to_string(),
            description: state.description.lines.join("\n").trim().to_string(),
            project: state.selected_project.clone(),
            status: state.status.clone(),
            priority: state.priority.clone(),
            labels: state.labels.clone(),
            available_at,
            due_on,
            is_epic: false,
        };
        let attachments = self.authoring.take_add_task_attachments();
        if let Err(error) = self.submit_created_task(draft, attachments).await {
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            return Err(error);
        }
        self.authoring.clear();
        Ok(())
    }

    async fn handle_text_submit(&mut self, route: OverlayRoute, value: String) -> Result<()> {
        match route.text_submit_route() {
            Some(TextSubmitRoute::AddTaskTitleToast) => {
                self.set_success(route.fallback_message(OverlaySubmitKind::Text))
            }
            Some(TextSubmitRoute::AddProject) => {
                let message = self.store.create_project(value).await?;
                self.restore_selection_after_mutation();
                self.set_success(message);
            }
            Some(TextSubmitRoute::AddLabel) => {
                let message = self.store.create_label(value).await?;
                self.set_success(message);
            }
            Some(TextSubmitRoute::RenameProjectName) => {
                self.submit_rename_project(value).await?;
            }
            Some(TextSubmitRoute::DeleteProjectNameConfirm) => {
                self.submit_delete_project_name(value).await?;
            }
            Some(TextSubmitRoute::EditTitle) => {
                self.submit_edit_title(value).await?;
            }
            Some(TextSubmitRoute::EditAvailability) => {
                self.submit_edit_availability(value).await?;
            }
            Some(TextSubmitRoute::EditDue) => {
                self.submit_edit_due(value).await?;
            }
            Some(TextSubmitRoute::ConflictManual) => {
                self.submit_manual_conflict_value(value).await?;
            }
            None => self.set_success(route.fallback_message(OverlaySubmitKind::Text)),
        }
        Ok(())
    }

    async fn handle_multiline_submit(&mut self, route: OverlayRoute, value: String) -> Result<()> {
        match route.multiline_submit_route() {
            Some(MultilineSubmitRoute::AddTaskDescription) => {
                if self.authoring.capture_add_task_fields(
                    self.authoring
                        .add_task_context()
                        .map(|context| context.title)
                        .unwrap_or_default(),
                    value,
                    AddTaskStep::Description,
                ) {
                    self.begin_add_task_step();
                }
            }
            Some(MultilineSubmitRoute::AddTaskNatural) => {
                self.submit_add_task_natural(value).await?;
            }
            Some(MultilineSubmitRoute::AddNote) => {
                self.submit_add_note(value).await?;
            }
            Some(MultilineSubmitRoute::EditDescription) => {
                self.submit_edit_description(value).await?;
            }
            Some(MultilineSubmitRoute::ConflictManual) => {
                self.submit_manual_conflict_value(value).await?;
            }
            None => self.set_success(route.fallback_message(OverlaySubmitKind::Multiline)),
        }
        Ok(())
    }

    async fn handle_picker_submit(
        &mut self,
        route: OverlayRoute,
        values: Vec<String>,
    ) -> Result<()> {
        match route.picker_submit_route() {
            Some(PickerSubmitRoute::AddTaskTitleProject) => {
                if self.authoring.apply_add_task_project(values) {
                    self.begin_add_task_step();
                }
            }
            Some(PickerSubmitRoute::AddTaskTitlePriority) => {
                if self.authoring.apply_add_task_priority(values) {
                    self.begin_add_task_step();
                }
            }
            Some(PickerSubmitRoute::AddTaskTitleLabels) => {
                self.submit_add_task_title_labels(values).await?;
            }
            Some(PickerSubmitRoute::EditStatus) => match values.first() {
                Some(status) => self.submit_edit_status(status.clone()).await?,
                None => {
                    self.set_warning("no matching status");
                    self.begin_status_picker();
                }
            },
            Some(PickerSubmitRoute::MoveToColumn) => match values.first() {
                Some(status) => self.move_tasks_to_column(status.clone()).await?,
                None => {
                    self.set_warning("no matching column");
                    self.begin_move_to_column();
                }
            },
            Some(PickerSubmitRoute::EditProject) => match values.first() {
                Some(project) => self.submit_edit_project(project.clone()).await?,
                None => {
                    self.set_warning("no matching project");
                    self.begin_edit_project();
                }
            },
            Some(PickerSubmitRoute::EditPriority) => match values.first() {
                Some(priority) => self.submit_edit_priority(priority.clone()).await?,
                None => {
                    self.set_warning("no matching priority");
                    self.begin_edit_priority();
                }
            },
            Some(PickerSubmitRoute::EditLabels) => {
                self.submit_edit_labels(values).await?;
            }
            Some(PickerSubmitRoute::EditLabelsMulti) => {
                self.submit_edit_labels_multi(values).await?;
            }
            Some(PickerSubmitRoute::FilterLabel) => {
                self.submit_filter_label(values).await?;
            }
            Some(PickerSubmitRoute::FilterPriority) => {
                self.submit_filter_priority(values).await?;
            }
            Some(PickerSubmitRoute::ScopeProject) => {
                self.submit_scope_project(values).await?;
            }
            Some(PickerSubmitRoute::RenameProjectPicker) => {
                self.submit_rename_project_picker(values);
            }
            Some(PickerSubmitRoute::DeleteProjectPicker) => {
                self.submit_delete_project_picker(values);
            }
            Some(PickerSubmitRoute::SwitchWorkspace) => {
                self.submit_switch_workspace(values).await?;
            }
            Some(PickerSubmitRoute::ConflictField) => {
                self.submit_conflict_field_picker(values).await?;
            }
            Some(PickerSubmitRoute::ConflictManual) => {
                if let Some(value) = values.first() {
                    self.submit_manual_conflict_value(value.clone()).await?;
                } else {
                    self.set_warning("no value selected");
                }
            }
            Some(PickerSubmitRoute::AddDependency) => match values.first() {
                Some(task_id) => self.submit_add_dependency(task_id.parse()?).await?,
                None => {
                    self.set_warning("no matching task");
                    self.begin_add_dependency().await?;
                }
            },
            Some(PickerSubmitRoute::RemoveDependency) => match values.first() {
                Some(task_id) => self.submit_remove_dependency(task_id.parse()?).await?,
                None => {
                    self.set_warning("no matching dependency");
                    self.begin_remove_dependency();
                }
            },
            None => self.set_success(route.fallback_message(OverlaySubmitKind::Picker)),
        }
        Ok(())
    }

    async fn handle_confirm_submit(&mut self, route: OverlayRoute) -> Result<()> {
        match route.confirm_submit_route() {
            Some(ConfirmSubmitRoute::ConflictConfirm) => {
                self.submit_confirmed_conflict_resolution().await?;
            }
            Some(ConfirmSubmitRoute::ConfigInit) => {
                self.submit_config_init()?;
            }
            Some(ConfirmSubmitRoute::DeleteProjectConfirm) => {
                self.submit_delete_project().await?;
            }
            Some(ConfirmSubmitRoute::DeleteTaskConfirm) => {
                let return_to_detail = self.detail_context;
                self.update_deleted(true).await?;
                self.detail_context = false;
                self.restore_detail_overlay(return_to_detail);
            }
            Some(ConfirmSubmitRoute::UpdateConfirm) => {
                self.confirm_update()?;
            }
            None => self.set_success(route.fallback_message(OverlaySubmitKind::Confirm)),
        }
        Ok(())
    }
}

use anyhow::Result;

use crate::operations::TaskDraft;
use crate::tui::app::App;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{
    AddTaskMode, AddTaskState, ConfirmIntent, MultilineIntent, OverlayState, OverlaySubmit,
    PickerIntent, TagComboboxIntent, TextIntent,
};

impl App {
    pub(super) async fn handle_overlay_submit(&mut self, submit: OverlaySubmit) -> Result<()> {
        match submit {
            OverlaySubmit::AddTask(state) => self.handle_add_task_submit(*state).await?,
            OverlaySubmit::Picker {
                intent,
                values,
                partial_values,
            } => {
                debug_assert!(partial_values.is_empty());
                self.handle_picker_submit(intent, values).await?;
            }
            OverlaySubmit::TagCombobox {
                intent,
                values,
                partial_values,
            } => {
                self.handle_tag_combobox_submit(intent, values, partial_values)
                    .await?;
            }
            OverlaySubmit::HeaderMenu { action } => self.submit_header_menu(action).await?,
            OverlaySubmit::Order { order } => self.submit_order_menu(order).await?,
            OverlaySubmit::Text { intent, value } => {
                self.handle_text_submit(intent, value).await?;
            }
            OverlaySubmit::ClearDate { intent } => {
                self.begin_clear_edit_value(intent).await?;
            }
            OverlaySubmit::Multiline { intent, value } => {
                self.handle_multiline_submit(intent, value).await?;
            }
            OverlaySubmit::Confirm { intent } => self.handle_confirm_submit(intent).await?,
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
        self.capture_add_task_state(&state);
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
        if let Err(error) = self.submit_created_task(draft).await {
            if !crate::tui::store::task_creation_committed(&error) {
                self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            }
            return Err(error);
        }
        Ok(())
    }

    async fn handle_text_submit(&mut self, intent: TextIntent, value: String) -> Result<()> {
        match intent {
            TextIntent::AddProject => {
                let message = self.store.create_project(value).await?;
                self.restore_selection_after_mutation();
                self.set_success(message);
            }
            TextIntent::AddLabel => {
                let message = self.store.create_label(value).await?;
                self.set_success(message);
            }
            TextIntent::RenameProject { project } => {
                self.submit_rename_project(project, value).await?;
            }
            TextIntent::ConfirmDeleteProject { project } => {
                self.submit_delete_project_name(project, value).await?;
            }
            TextIntent::EditTitle { selection } => {
                self.submit_edit_title(selection, value).await?;
            }
            TextIntent::EditAvailability { selection, mixed } => {
                self.submit_edit_availability(selection, mixed, value)
                    .await?;
            }
            TextIntent::EditDue { selection, mixed } => {
                self.submit_edit_due(selection, mixed, value).await?;
            }
            TextIntent::ResolveConflictManually { target } => {
                self.submit_manual_conflict_value(target, value).await?;
            }
        }
        Ok(())
    }

    async fn handle_multiline_submit(
        &mut self,
        intent: MultilineIntent,
        value: String,
    ) -> Result<()> {
        match intent {
            MultilineIntent::AddTaskDescription => {
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
            MultilineIntent::AddTaskNatural => self.submit_add_task_natural(value).await?,
            MultilineIntent::AddNote {
                task_id,
                display_ref,
            } => {
                self.submit_add_note(task_id, display_ref, value).await?;
            }
            MultilineIntent::EditDescription { selection } => {
                self.submit_edit_description(selection, value).await?;
            }
            MultilineIntent::ResolveConflictManually { target } => {
                self.submit_manual_conflict_value(target, value).await?;
            }
        }
        Ok(())
    }

    async fn handle_picker_submit(
        &mut self,
        intent: PickerIntent,
        values: Vec<String>,
    ) -> Result<()> {
        match intent {
            PickerIntent::AddTaskProject => {
                if self.authoring.apply_add_task_project(values) {
                    self.begin_add_task_step();
                }
            }
            PickerIntent::AddTaskStatus => {
                if let Some(value) = values.first() {
                    self.authoring.apply_add_task_status(value);
                    self.begin_add_task_step();
                }
            }
            PickerIntent::AddTaskPriority => {
                if self.authoring.apply_add_task_priority(values) {
                    self.begin_add_task_step();
                }
            }
            PickerIntent::MoveToColumn { selection } => match values.first() {
                Some(status) => {
                    self.move_tasks_to_column(selection, status.clone()).await?;
                }
                None => {
                    self.set_warning("no matching column");
                    self.open_move_to_column_picker(selection);
                }
            },
            PickerIntent::EditProject { selection, mixed } => match values.first() {
                Some(project) => {
                    self.submit_edit_project(selection, mixed, project.clone())
                        .await?;
                }
                None => {
                    self.set_warning("no matching project");
                    self.open_edit_project_picker(selection);
                }
            },
            PickerIntent::EditPriority { selection, mixed } => match values.first() {
                Some(priority) => {
                    self.submit_edit_priority(selection, mixed, priority.clone())
                        .await?;
                }
                None => {
                    self.set_warning("no matching priority");
                    self.open_edit_priority_picker_for_selection(selection);
                }
            },
            PickerIntent::FilterLabel => self.submit_filter_label(values).await?,
            PickerIntent::FilterPriority => self.submit_filter_priority(values).await?,
            PickerIntent::ScopeProject => self.submit_scope_project(values).await?,
            PickerIntent::RenameProject => self.submit_rename_project_picker(values),
            PickerIntent::DeleteProject => self.submit_delete_project_picker(values),
            PickerIntent::SwitchWorkspace => self.submit_switch_workspace(values).await?,
            intent @ (PickerIntent::PickConflictVariant { .. }
            | PickerIntent::PickConflictManual { .. }) => {
                self.submit_conflict_field_picker(intent, values).await?;
            }
            PickerIntent::ResolveConflictManually { target } => {
                if let Some(value) = values.first() {
                    self.submit_manual_conflict_value(target, value.clone())
                        .await?;
                } else {
                    self.set_warning("no value selected");
                }
            }
            PickerIntent::RemoveDependency { selection } => match values.first() {
                Some(depends_on_task_id) => {
                    self.submit_remove_dependency(selection, depends_on_task_id.parse()?)
                        .await?;
                }
                None => {
                    self.set_warning("no matching dependency");
                    self.open_remove_dependency_picker(selection);
                }
            },
        }
        Ok(())
    }

    async fn handle_tag_combobox_submit(
        &mut self,
        intent: TagComboboxIntent,
        values: Vec<String>,
        partial_values: Vec<String>,
    ) -> Result<()> {
        match intent {
            TagComboboxIntent::AddTaskLabels => {
                self.submit_add_task_title_labels(values).await?;
            }
            TagComboboxIntent::EditLabels { selection } => {
                self.submit_edit_labels(selection, values).await?;
            }
            TagComboboxIntent::EditLabelsMulti { selection } => {
                self.submit_edit_labels_multi(selection, values, partial_values)
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_confirm_submit(&mut self, intent: ConfirmIntent) -> Result<()> {
        match intent {
            ConfirmIntent::ResolveConflict { target, value } => {
                self.submit_confirmed_conflict_resolution(target, value)
                    .await?;
            }
            ConfirmIntent::InitializeConfig { path } => self.submit_config_init(path)?,
            ConfirmIntent::DeleteProject { project } => {
                self.submit_delete_project(project).await?;
            }
            ConfirmIntent::DeleteTasks { selection } => {
                self.submit_delete_selection(selection).await?;
            }
            ConfirmIntent::DeleteAttachment { attachment_id } => {
                self.submit_delete_attachment(attachment_id).await?;
            }
            ConfirmIntent::ClearAvailability { selection } => {
                self.submit_clear_availability(selection).await?;
            }
            ConfirmIntent::ClearDue { selection } => {
                self.submit_clear_due(selection).await?;
            }
            ConfirmIntent::InstallUpdate { plan } => self.confirm_update(plan)?,
        }
        Ok(())
    }
}

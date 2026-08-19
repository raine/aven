use anyhow::Result;

use crate::operations::TaskDraft;
use crate::tui::app::App;
use crate::tui::authoring::{ADD_TASK_TITLE_PROJECT_TITLE, AddTaskStep};
use crate::tui::overlay::{
    AddTaskMode, AddTaskState, ConfirmIntent, MultilineIntent, OverlayState, OverlaySubmit,
    PickerIntent, TagComboboxIntent, TextIntent,
};

impl App {
    pub(super) async fn handle_overlay_submit(&mut self, submit: OverlaySubmit) -> Result<()> {
        match submit {
            OverlaySubmit::AddTask(state) => self.handle_add_task_submit(*state).await?,
            OverlaySubmit::CreateAddTaskProject { state, name } => {
                self.handle_create_add_task_project(state, name).await?;
            }
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

    async fn handle_create_add_task_project(
        &mut self,
        mut state: Box<AddTaskState>,
        name: String,
    ) -> Result<()> {
        match self.store.create_project(name.clone()).await {
            Ok(outcome) => {
                state.selected_project = Some(outcome.project_key.clone());
                state.project = outcome.project_key.clone();
                state.refresh_recurrence_preview();
                self.overlay = Some(OverlayState::AddTask(state));
                self.set_success(format!("created project {}", outcome.project_key));
            }
            Err(error) => {
                let mut picker = crate::tui::overlay::PickerState::new(
                    PickerIntent::AddTaskProject,
                    ADD_TASK_TITLE_PROJECT_TITLE,
                    self.store
                        .project_picker_items(state.selected_project.as_deref()),
                    false,
                );
                picker.filter = crate::tui::overlay::LineEdit::new(name);
                crate::tui::overlay::sync_project_creation_item(&mut picker);
                crate::tui::overlay::normalize_picker_selection(&mut picker);
                state.mode = AddTaskMode::Picker {
                    field: AddTaskStep::Project,
                    state: picker,
                };
                self.overlay = Some(OverlayState::AddTask(state));
                self.set_error(format!("{error:#}"));
            }
        }
        Ok(())
    }

    async fn handle_add_task_submit(&mut self, mut state: AddTaskState) -> Result<()> {
        let create_more = std::mem::take(&mut state.create_more);
        if let Some(error) = state.schedule_error.clone() {
            state.focus = AddTaskStep::Schedule;
            state.schedule_validation_requested = true;
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning(error);
            return Ok(());
        }
        let title = state.title.text.trim();
        if title.is_empty() {
            state.focus = AddTaskStep::Title;
            state.mode = AddTaskMode::Compose;
            state.title_error = true;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning("task title is required");
            return Ok(());
        }

        let recurrence_schedule = match state.recurrence_schedule() {
            Ok(value) => value,
            Err(error) => {
                let message = format!("{error:#}");
                state.focus = if message.contains("invalid-repeat-at") {
                    AddTaskStep::RepeatAt
                } else if message.contains("invalid-repeat-due") {
                    AddTaskStep::RepeatDue
                } else if message.contains("invalid-recurrence-date") {
                    AddTaskStep::RepeatStartOn
                } else {
                    AddTaskStep::RepeatRule
                };
                state.mode = AddTaskMode::Compose;
                state.recurrence_error = Some(message.clone());
                self.overlay = Some(OverlayState::AddTask(Box::new(state)));
                self.set_warning(message);
                return Ok(());
            }
        };
        let recurring = state.template_schedule.is_some() || recurrence_schedule.is_some();
        if state.is_epic && !self.authoring.is_standalone_add_task() {
            state.focus = AddTaskStep::Epic;
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning("epic children cannot be epic containers");
            return Ok(());
        }
        if recurring && state.is_epic {
            state.focus = AddTaskStep::Epic;
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning("recurring tasks cannot be epic containers");
            return Ok(());
        }
        if recurring
            && !matches!(
                crate::choices::TaskStatus::parse(state.effective_status()),
                Ok(status) if status.is_open()
            )
        {
            state.focus = AddTaskStep::Status;
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning(
                "recurring tasks require an open initial status: inbox, backlog, todo, or active",
            );
            return Ok(());
        }
        if recurrence_schedule.is_some()
            && (!state.available_at.text.trim().is_empty() || !state.due_on.text.trim().is_empty())
        {
            state.focus = AddTaskStep::RepeatAt;
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning(
                "recurring tasks use recurrence availability and due policy; clear Available and Due",
            );
            return Ok(());
        }
        if recurrence_schedule.is_some()
            && state.recurrence_series_id.is_none()
            && state.selected_project.is_none()
            && state.inferred_project.is_none()
        {
            state.focus = AddTaskStep::Project;
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning("Choose a project for this recurring task");
            return Ok(());
        }
        if recurrence_schedule.is_some() && self.authoring.add_task_has_pending_attachments() {
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(Box::new(state)));
            self.set_warning("create the recurring task before adding occurrence attachments");
            return Ok(());
        }

        if let Some(series_id) = state.recurrence_series_id.clone() {
            let schedule = recurrence_schedule
                .as_ref()
                .expect("recurrence template editor retains its schedule");
            let selected_task_id = self
                .store
                .selected_task(self.list.selected_task())
                .map(|item| item.task.id.clone());
            let message = self
                .store
                .update_recurrence_template(
                    &series_id,
                    aven_core::operations::RecurrenceTemplateUpdate {
                        title: Some(title.to_string()),
                        description: Some(state.description.lines.join("\n").trim().to_string()),
                        project: state.selected_project.clone(),
                        priority: Some(state.priority.value().to_string()),
                        initial_status: Some(state.effective_status().to_string()),
                        labels: Some(state.labels.clone()),
                        set_metadata: Vec::new(),
                        remove_metadata: Vec::new(),
                        available_local_time: Some(schedule.available_local_time),
                        due_policy: Some(schedule.due_policy),
                    },
                    selected_task_id.as_ref(),
                )
                .await?;
            self.list.select_task(message.selected);
            self.set_success(message.message);
            self.authoring.clear();
            return Ok(());
        }

        if let Some(schedule) = recurrence_schedule {
            let draft = crate::tui::store::recurrence_draft(
                title.to_string(),
                state.description.lines.join("\n").trim().to_string(),
                state
                    .selected_project
                    .clone()
                    .or_else(|| state.inferred_project.clone()),
                state.priority.value().to_string(),
                state.effective_status().to_string(),
                state.labels.clone(),
                schedule,
            );
            let (message, selected) = self
                .store
                .create_recurrence_series(draft, self.list.selected_task())
                .await?;
            self.list.select_task(selected);
            self.preserve_or_restore_sidebar_selection();
            self.prune_task_marks();
            self.set_success(message.clone());
            if self.intake.view().add_task_only {
                self.intake.set_message(message);
                self.should_quit = true;
            }
            self.authoring.clear();
            return Ok(());
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
        state.create_more = create_more;
        self.capture_add_task_state(&state);
        state.create_more = false;
        let draft = TaskDraft {
            title: title.to_string(),
            description: state.description.lines.join("\n").trim().to_string(),
            project: state.selected_project.clone(),
            status: state.effective_status().to_string(),
            priority: state.priority.value().to_string(),
            source: crate::choices::TaskSource::Tui,
            labels: state.labels.clone(),
            metadata: Vec::new(),
            available_at,
            due_on,
            is_epic: state.is_epic,
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
                let outcome = self.store.create_project(value).await?;
                self.restore_selection_after_mutation();
                self.set_success(format!("created project {}", outcome.project_key));
            }
            TextIntent::AddProjectPath { project } => {
                self.submit_add_project_path(project, value).await?;
            }
            TextIntent::AddLabel => {
                let message = self.store.create_label(value).await?;
                self.set_success(message);
            }
            TextIntent::RenameLabel { label } => {
                self.submit_rename_label(label, value).await?;
            }
            TextIntent::ConfirmDeleteLabel {
                label,
                task_count,
                series_count,
            } => {
                self.submit_delete_label_name(label, task_count, series_count, value);
            }
            TextIntent::AddWorkspace => {
                self.submit_add_workspace(value).await?;
            }
            TextIntent::RenameWorkspace { workspace } => {
                self.submit_rename_workspace(workspace, value).await?;
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
            TextIntent::SaveAttachment {
                attachment_id,
                filename,
                scroll,
            } => {
                self.submit_save_attachment(attachment_id, filename, scroll, value)
                    .await?;
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
            MultilineIntent::EditNote {
                task_id,
                display_ref,
                note_id,
            } => {
                self.submit_edit_note(task_id, display_ref, note_id, value)
                    .await?;
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
                    self.authoring.apply_add_task_status_choice(value);
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
                Some(project)
                    if crate::tui::store::create_project_picker_name(project).is_some() =>
                {
                    let name = crate::tui::store::create_project_picker_name(project)
                        .expect("guarded project creation value")
                        .to_string();
                    match self.store.create_project(name.clone()).await {
                        Ok(outcome) => {
                            self.submit_edit_project(selection, mixed, outcome.project_key)
                                .await?;
                        }
                        Err(error) => {
                            self.open_edit_project_picker(selection);
                            if let Some(OverlayState::Picker(picker)) = self.overlay.as_mut() {
                                picker.filter = crate::tui::overlay::LineEdit::new(name);
                                crate::tui::overlay::sync_project_creation_item(picker);
                                crate::tui::overlay::normalize_picker_selection(picker);
                            }
                            self.set_error(format!("{error:#}"));
                        }
                    }
                }
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
            PickerIntent::EditEpic { selection, mixed } => match values.first() {
                Some(value) => {
                    self.submit_edit_epic(selection, mixed, value.clone())
                        .await?;
                }
                None => {
                    self.set_warning("choose whether this task is an epic container");
                    self.open_edit_epic_picker(selection);
                }
            },
            PickerIntent::FilterLabel => self.submit_filter_label(values).await?,
            PickerIntent::FilterPriority => self.submit_filter_priority(values).await?,
            PickerIntent::ScopeProject => self.submit_scope_project(values).await?,
            PickerIntent::RenameProject => self.submit_rename_project_picker(values),
            PickerIntent::DeleteProject => self.submit_delete_project_picker(values),
            PickerIntent::AddProjectPath => self.submit_add_project_path_picker(values),
            PickerIntent::RemoveProjectPath => {
                self.submit_remove_project_path_picker(values).await;
            }
            PickerIntent::RemoveProjectPathValue { project } => {
                self.submit_remove_project_path_value(project, values);
            }
            PickerIntent::BrowseLabels => self.submit_browse_label(values),
            PickerIntent::LabelActions { label } => {
                self.submit_label_action(label, values).await?;
            }
            PickerIntent::RenameLabel => self.submit_rename_label_picker(values),
            PickerIntent::DeleteLabel => self.submit_delete_label_picker(values).await?,
            PickerIntent::SwitchWorkspace => self.submit_switch_workspace(values).await?,
            PickerIntent::RenameWorkspace => self.submit_rename_workspace_picker(values),
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
            PickerIntent::RecurrenceActions { target } => {
                self.submit_recurrence_action(Some(target), values.first().map(String::as_str))
                    .await?;
            }
            PickerIntent::StopRecurrence { target } => {
                self.submit_stop_recurrence(Some(target), values.first().map(String::as_str))
                    .await?;
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
            ConfirmIntent::RemoveProjectPath { project, path } => {
                self.submit_remove_project_path(project, path).await?;
            }
            ConfirmIntent::DeleteLabel { label } => {
                self.submit_delete_label(label).await?;
            }
            ConfirmIntent::DeleteTasks { selection } => {
                self.submit_delete_selection(selection).await?;
            }
            ConfirmIntent::DeleteNote { task_id, note_id } => {
                self.submit_delete_note(task_id, note_id).await?;
            }
            ConfirmIntent::DeleteFocusedTask { selection } => {
                self.submit_delete_focused_task(selection).await?;
            }
            ConfirmIntent::UnlinkDependency {
                selection,
                depends_on_task_id,
            } => {
                self.submit_remove_dependency(selection, depends_on_task_id)
                    .await?;
            }
            ConfirmIntent::UnlinkEpicChild { epic_id, child_id } => {
                self.submit_unlink_epic_child(epic_id, child_id).await?;
            }
            ConfirmIntent::DeleteAttachment { attachment_id } => {
                self.submit_delete_attachment(attachment_id).await?;
            }
            ConfirmIntent::PromoteTaskForChild { epic } => {
                self.open_add_epic_child_search(epic);
            }
            ConfirmIntent::CreateTaskGist { task_id } => {
                self.submit_create_task_gist(task_id).await;
            }
            ConfirmIntent::ClearAvailability { selection } => {
                self.submit_clear_availability(selection).await?;
            }
            ConfirmIntent::ClearDue { selection } => {
                self.submit_clear_due(selection).await?;
            }
        }
        Ok(())
    }
}

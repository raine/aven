use anyhow::Result;

use crate::config::resolve_blob_dir;
use crate::operations::TaskDraft;
use crate::tui::app::{App, Notification};
use crate::tui::app_intake::{IntakeCompletion, IntakePoll, NaturalRetry};
use crate::tui::authoring::{
    ADD_NOTE_TITLE, ADD_TASK_LABELS_TITLE, ADD_TASK_TITLE_PROJECT_TITLE, AddTaskOrigin, AddTaskStep,
};
use crate::tui::natural_add_runtime::task_intake_log_path;
use crate::tui::overlay::{
    AddTaskMode, AddTaskState, LineEdit, MultilineInputState, MultilineIntent, OverlayState,
    PickerIntent, PickerState, ScheduleEditorField, TagComboboxIntent,
};
use crate::tui::platform::edit_text_externally;
use crate::tui::store::TaskScope;

pub(crate) const ADD_TASK_NATURAL_TITLE: &str = "Add task: natural language";

impl App {
    pub(super) async fn begin_add_task(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        let active_project = match &self.store.view_state.scope {
            TaskScope::Project(project) => Some(project.clone()),
            TaskScope::Workspace => None,
        };
        let inferred_project = if active_project.is_none() {
            self.store.inferred_add_project().await?
        } else {
            None
        };
        self.authoring
            .begin_add_task(active_project, inferred_project);
        self.apply_default_add_task_recurrence()?;
        self.begin_add_task_title();
        Ok(())
    }

    fn apply_default_add_task_recurrence(&mut self) -> Result<()> {
        let defaults = crate::commands::recurrence_schedule("daily", None, None, None, None)?;
        self.authoring.apply_add_task_recurrence(
            None,
            None,
            String::new(),
            String::new(),
            "same-day".to_string(),
            defaults.timezone.to_string(),
            defaults.start_on.to_string(),
        );
        Ok(())
    }

    pub(super) fn begin_add_task_title(&mut self) {
        self.begin_add_task_overlay();
    }

    fn begin_add_task_overlay(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        let selected_project = self.authoring.selected_add_task_project().flatten();
        let attachments = self.authoring.add_task_attachment_summaries();
        let inferred_project = (selected_project.is_none() && context.project != "no project")
            .then(|| context.project.clone());
        self.overlay = Some(OverlayState::AddTask(Box::new(AddTaskState {
            title: LineEdit::new(context.title),
            description: MultilineInputState::from_value(
                MultilineIntent::AddTaskDescription,
                "Add task: description",
                "",
                context.description,
            ),
            focus: context.step,
            project: context.project,
            inferred_project,
            selected_project: selected_project.clone(),
            initial_project: selected_project,
            status: context.status,
            priority: context.priority,
            labels: context.labels,
            is_epic: context.is_epic,
            create_more: context.create_more,
            create_more_available: context.create_more_available
                && !self.intake.view().add_task_only,
            available_at: LineEdit::new(context.available_at),
            due_on: LineEdit::new(context.due_on),
            schedule_input: LineEdit::new(context.schedule_input),
            schedule_error: None,
            schedule_validation_requested: false,
            selected_attachment: attachments.len().saturating_sub(1),
            attachments,
            recurrence_series_id: context.recurrence_series_id,
            template_schedule: context.template_schedule,
            repeat_rule: LineEdit::new(context.repeat_rule),
            repeat_at: LineEdit::new(context.repeat_at),
            repeat_due: context.repeat_due,
            time_zone: context.time_zone,
            repeat_start_on: LineEdit::new(context.repeat_start_on),
            schedule_expanded: context.schedule_expanded,
            recurrence_preview: Vec::new(),
            recurrence_error: None,
            mode: crate::tui::overlay::AddTaskMode::Compose,
            title_error: false,
        })));
        if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
            state.refresh_recurrence_preview();
        }
    }

    pub(super) fn begin_add_task_step(&mut self) {
        self.begin_add_task_overlay();
    }

    pub(super) fn open_add_task_description_editor(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        self.prepare_terminal_transition();
        match edit_text_externally(
            context.description.clone(),
            "description.md",
            self.terminal_mouse_capture,
        ) {
            Ok(value) => {
                self.authoring.capture_add_task_fields(
                    context.title,
                    value,
                    AddTaskStep::Description,
                );
                self.begin_add_task_step();
            }
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.begin_add_task_step();
            }
        }
    }

    pub(super) fn capture_add_task_state(&mut self, state: &AddTaskState) -> bool {
        let captured = self.authoring.capture_add_task_fields(
            state.title.text.clone(),
            state.description.lines.join("\n"),
            state.focus,
        );
        if captured {
            self.authoring
                .apply_add_task_project(state.selected_project.clone().into_iter().collect());
            self.authoring
                .capture_add_task_choices(state.status.clone(), state.priority.clone());
            self.authoring.apply_add_task_labels(state.labels.clone());
            self.authoring.apply_add_task_epic(state.is_epic);
            self.authoring
                .apply_add_task_create_more(state.create_more && state.create_more_available);
            self.authoring
                .apply_add_task_available_at(state.available_at.text.clone());
            self.authoring
                .apply_add_task_due_on(state.due_on.text.clone());
            self.authoring.apply_add_task_recurrence(
                state.recurrence_series_id.clone(),
                state.template_schedule.clone(),
                state.repeat_rule.text.clone(),
                state.repeat_at.text.clone(),
                state.repeat_due.clone(),
                state.time_zone.clone(),
                state.repeat_start_on.text.clone(),
            );
            self.authoring
                .apply_add_task_schedule_input(state.schedule_input.text.clone());
            self.authoring
                .set_add_task_schedule_expanded(state.schedule_expanded);
        }
        captured
    }

    pub(super) fn set_add_task_status(&mut self, value: &str) {
        if let Some(choice) = self.authoring.apply_add_task_status_choice(value) {
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.status = choice;
            } else {
                self.begin_add_task_step();
            }
        }
    }

    pub(super) fn set_add_task_priority(&mut self, value: &str) {
        if let Some(choice) = self.authoring.apply_add_task_priority_value(value) {
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.priority = choice;
            } else {
                self.begin_add_task_step();
            }
        }
    }

    pub(super) fn open_focused_add_task_control(&mut self) {
        let Some(OverlayState::AddTask(mut state)) = self.overlay.take() else {
            return;
        };
        if !state.is_step_editable(state.focus) {
            state.mode = AddTaskMode::Compose;
            self.overlay = Some(OverlayState::AddTask(state));
            return;
        }
        state.mode = match state.focus {
            AddTaskStep::Project => AddTaskMode::Picker {
                field: state.focus,
                state: PickerState::new(
                    PickerIntent::AddTaskProject,
                    ADD_TASK_TITLE_PROJECT_TITLE,
                    self.store
                        .project_picker_items(state.selected_project.as_deref()),
                    false,
                ),
            },
            AddTaskStep::Status => AddTaskMode::Picker {
                field: state.focus,
                state: PickerState::new(
                    PickerIntent::AddTaskStatus,
                    "Add task: status",
                    self.store.add_task_status_picker_items(
                        state.effective_status(),
                        state.automatic_status(),
                        state.status_is_automatic(),
                    ),
                    false,
                ),
            },
            AddTaskStep::Priority => AddTaskMode::Picker {
                field: state.focus,
                state: PickerState::new(
                    PickerIntent::AddTaskPriority,
                    "Add task: priority",
                    self.store.priority_picker_items(state.priority.value()),
                    false,
                ),
            },
            AddTaskStep::Labels => {
                let OverlayState::TagCombobox(labels) = OverlayState::tag_combobox(
                    TagComboboxIntent::AddTaskLabels,
                    ADD_TASK_LABELS_TITLE,
                    task_label_options(&self.store.labels, &state.labels),
                    state.labels.clone(),
                ) else {
                    unreachable!();
                };
                AddTaskMode::Labels(labels)
            }
            AddTaskStep::Epic => {
                state.is_epic = !state.is_epic;
                AddTaskMode::Compose
            }
            AddTaskStep::Schedule => {
                let mut editor = state.schedule_editor(ScheduleEditorField::Mode);
                editor.refresh();
                AddTaskMode::Schedule(editor)
            }
            AddTaskStep::RepeatRule => AddTaskMode::Compose,

            _ => AddTaskMode::Compose,
        };
        self.overlay = Some(OverlayState::AddTask(state));
    }

    pub(super) fn begin_add_note(&mut self) {
        self.pending_shortcut.clear();
        let Some(selection) = self.resolve_task_selection() else {
            self.set_info("no selected task for note");
            return;
        };
        if !selection.is_single() {
            self.set_info(format!(
                "note requires one task · {} tasks marked",
                selection.len()
            ));
            return;
        }
        let item = &selection.targets()[0];
        self.overlay = Some(OverlayState::blank_multiline_input(
            MultilineIntent::AddNote {
                task_id: item.task.id.clone(),
                display_ref: item.display_ref.clone(),
            },
            ADD_NOTE_TITLE,
            "note body:",
        ));
    }

    pub(super) async fn submit_add_task_title_labels(&mut self, labels: Vec<String>) -> Result<()> {
        if self.authoring.apply_add_task_labels(labels) {
            self.begin_add_task_step();
        }
        Ok(())
    }

    pub(super) fn begin_add_task_natural(&mut self) {
        self.begin_add_task_natural_with_value(String::new());
    }

    fn begin_add_task_natural_with_value(&mut self, value: String) {
        self.overlay = Some(OverlayState::multiline_input(
            MultilineIntent::AddTaskNatural,
            ADD_TASK_NATURAL_TITLE,
            "",
            value,
        ));
    }

    pub(super) async fn submit_add_task_title_natural(
        &mut self,
        title: String,
        description: String,
    ) -> Result<()> {
        let value = add_task_natural_intake(&title, &description);
        if self.intake.view().add_task_only && !self.authoring.add_task_has_pending_attachments() {
            self.submit_add_task_only_natural(value, NaturalRetry::AddTask)
                .await
        } else {
            self.submit_add_task_natural_with_retry(value, NaturalRetry::AddTask, true)
                .await
        }
    }

    async fn submit_add_task_only_natural(
        &mut self,
        value: String,
        retry: NaturalRetry,
    ) -> Result<()> {
        let raw = value.trim();
        if raw.is_empty() {
            self.set_warning("task description is required");
            self.retry_add_task_natural(value, retry);
            return Ok(());
        }
        let project = self.add_task_project_context();
        self.intake.start_detached(
            raw,
            &self.store.active_workspace.id,
            project.as_deref(),
            false,
        )?;
        self.authoring.clear();
        self.intake.set_message("adding task in background");
        self.should_quit = true;
        Ok(())
    }

    pub(super) async fn submit_add_task_natural(&mut self, value: String) -> Result<()> {
        if self.intake.view().add_task_only && !self.authoring.add_task_has_pending_attachments() {
            self.submit_add_task_only_natural(value, NaturalRetry::Dialog)
                .await
        } else {
            self.submit_add_task_natural_with_retry(value, NaturalRetry::Dialog, false)
                .await
        }
    }

    async fn submit_add_task_natural_with_retry(
        &mut self,
        value: String,
        retry: NaturalRetry,
        create_on_success: bool,
    ) -> Result<()> {
        let raw = value.trim();
        if raw.is_empty() {
            self.set_warning("task description is required");
            self.retry_add_task_natural(value, retry);
            return Ok(());
        }
        let project = self.add_task_project_context();
        if create_on_success
            && !self.authoring.add_task_has_pending_attachments()
            && self.authoring.is_standalone_add_task()
        {
            self.intake.start_detached(
                raw,
                &self.store.active_workspace.id,
                project.as_deref(),
                true,
            )?;
            self.authoring.clear();
            self.overlay = None;
            self.set_info("adding task in background");
            return Ok(());
        }
        self.notification = Some(Notification::loading("parsing task with LLM"));
        self.intake.start(
            &self.store,
            raw.to_string(),
            project,
            retry,
            value.clone(),
            create_on_success,
        );
        self.retry_add_task_natural(value, retry);
        Ok(())
    }

    pub(super) async fn poll_pending_task_intake(&mut self) -> Result<bool> {
        match self.intake.poll().await? {
            IntakePoll::Unchanged => Ok(false),
            IntakePoll::Ready { failed } => {
                if failed {
                    self.set_error("task intake failed");
                }
                Ok(true)
            }
            IntakePoll::Completed(completion) => {
                self.finish_ready_task_intake(*completion).await?;
                Ok(true)
            }
        }
    }

    async fn finish_ready_task_intake(&mut self, ready: IntakeCompletion) -> Result<()> {
        match ready.outcome {
            Ok(intake) if ready.create_on_success => {
                if intake.recurrence.is_some() && self.authoring.add_task_has_pending_attachments()
                {
                    if self.authoring.apply_task_intake_result(intake) {
                        self.set_warning(
                            "recurring tasks cannot include occurrence attachments; review the task",
                        );
                        self.begin_add_task_step();
                    }
                } else {
                    self.submit_created_intake(intake).await?;
                    self.authoring.clear();
                }
            }
            Ok(intake) => {
                if self.authoring.apply_task_intake_result(intake) {
                    self.set_success("parsed task draft, review and save");
                    self.begin_add_task_step();
                }
            }
            Err(error) => {
                let log_path = task_intake_log_path();
                tracing::warn!(error = %error, "task intake failed");
                self.set_error(format!(
                    "task intake failed: {error:#}; logged to {}",
                    log_path.display()
                ));
                self.retry_add_task_natural(ready.value, ready.retry);
            }
        }
        Ok(())
    }

    fn retry_add_task_natural(&mut self, value: String, retry: NaturalRetry) {
        match retry {
            NaturalRetry::AddTask => self.begin_add_task_step(),
            NaturalRetry::Dialog => self.begin_add_task_natural_with_value(value),
        }
    }

    fn add_task_project_context(&self) -> Option<String> {
        self.authoring
            .selected_add_task_project()
            .flatten()
            .or_else(|| match &self.store.view_state.scope {
                TaskScope::Project(project) => Some(project.clone()),
                TaskScope::Workspace => None,
            })
    }

    async fn submit_created_intake(
        &mut self,
        intake: crate::task_intake::TaskIntakeResult,
    ) -> Result<()> {
        if intake.recurrence.is_none() {
            return self.submit_created_task(intake.task).await;
        }
        let draft = intake
            .into_recurrence_draft()
            .expect("recurring intake has a recurrence schedule");
        let (message, selected) = self
            .store
            .create_recurrence_series(draft, self.list.selected_task())
            .await?;
        self.list.select_task(selected);
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        if selected.is_none() {
            self.restore_selection_after_mutation();
        }
        self.set_success(message.clone());
        if self.intake.view().add_task_only {
            self.intake.set_message(message);
            self.should_quit = true;
        }
        Ok(())
    }

    pub(super) async fn submit_created_task(&mut self, draft: TaskDraft) -> Result<()> {
        let context = self
            .authoring
            .submission_context()
            .expect("task submission requires an active authoring flow");
        let current_selected = self.list.selected_task();
        match context.origin {
            AddTaskOrigin::EpicChild { epic, .. } => {
                let result = if context.attachments.is_empty() {
                    self.store
                        .create_task_for_epic(draft, current_selected, &epic)
                        .await
                } else {
                    let db_path = self
                        .intake
                        .db_path()
                        .ok_or_else(|| anyhow::anyhow!("database path is not available"))?;
                    let blob_dir = resolve_blob_dir(db_path, self.intake.config())?;
                    self.store
                        .create_task_with_attachments_for_epic(
                            draft,
                            current_selected,
                            &blob_dir,
                            self.intake.config().local.attachment_lifecycle.policy(),
                            context.attachments,
                            &epic,
                        )
                        .await
                };
                let (message, selected, task_id) = match result {
                    Ok(created) => created,
                    Err(error) if crate::tui::store::task_creation_committed(&error) => {
                        self.authoring.clear();
                        return Err(error);
                    }
                    Err(error) => {
                        self.set_error(format!("{error:#}"));
                        self.begin_add_task_step();
                        return Ok(());
                    }
                };
                let selected_epic = if let Some(index) = self
                    .store
                    .tasks
                    .iter()
                    .position(|item| item.task.id == epic.epic_id)
                {
                    Some(index)
                } else {
                    match self.store.load_task_item(&epic.epic_id).await {
                        Ok(Some(item)) => {
                            self.store.show_exact_task(item);
                            Some(0)
                        }
                        _ => selected,
                    }
                };
                self.list.select_task(selected_epic);
                self.show_detail(0);
                if let Some(detail) = self.detail.state_mut() {
                    detail.set_focused_target(Some(crate::tui::app::DetailTargetId::Task {
                        section: crate::tui::app::DetailSection::EpicChildren,
                        task_id,
                    }));
                    detail.set_removed_epic_child(None);
                    detail.set_scroll(0);
                }
                self.authoring.clear();
                self.set_mutation_success(message);
            }
            AddTaskOrigin::Standalone if context.create_more => {
                let completion = if context.attachments.is_empty() {
                    self.store
                        .create_task_completion(draft, current_selected)
                        .await?
                } else {
                    let db_path = self
                        .intake
                        .db_path()
                        .ok_or_else(|| anyhow::anyhow!("database path is not available"))?;
                    let blob_dir = resolve_blob_dir(db_path, self.intake.config())?;
                    self.store
                        .create_task_with_attachments_completion(
                            draft,
                            current_selected,
                            &blob_dir,
                            self.intake.config().local.attachment_lifecycle.policy(),
                            context.attachments,
                        )
                        .await?
                };
                self.list.select_task(completion.selected);
                if completion.refresh_error.is_none() {
                    self.preserve_or_restore_sidebar_selection();
                    self.prune_task_marks();
                    if completion.selected.is_none() {
                        self.restore_selection_after_mutation();
                    }
                }
                let message = completion.message;
                let refresh_error = completion.refresh_error;
                let reset = self.authoring.reset_after_created_task();
                debug_assert!(reset, "repeat-entry creation retains an active flow");
                self.apply_default_add_task_recurrence()?;
                self.begin_add_task_step();
                if let Some(error) = refresh_error {
                    self.set_warning(format!("{message}; list refresh failed: {error:#}"));
                } else {
                    self.set_mutation_success(message);
                }
                return Ok(());
            }
            AddTaskOrigin::Standalone => {
                let result = if context.attachments.is_empty() {
                    self.store.create_task(draft, current_selected).await
                } else {
                    let db_path = self
                        .intake
                        .db_path()
                        .ok_or_else(|| anyhow::anyhow!("database path is not available"))?;
                    let blob_dir = resolve_blob_dir(db_path, self.intake.config())?;
                    self.store
                        .create_task_with_attachments(
                            draft,
                            current_selected,
                            &blob_dir,
                            self.intake.config().local.attachment_lifecycle.policy(),
                            context.attachments,
                        )
                        .await
                };
                let (message, selected) = match result {
                    Ok(created) => created,
                    Err(error) => {
                        if crate::tui::store::task_creation_committed(&error) {
                            self.authoring.clear();
                        }
                        return Err(error);
                    }
                };
                self.authoring.clear();
                self.list.select_task(selected);
                self.preserve_or_restore_sidebar_selection();
                self.prune_task_marks();
                if selected.is_none() {
                    self.restore_selection_after_mutation();
                }
                self.set_mutation_success(message.clone());
                if self.intake.view().add_task_only {
                    self.intake.set_message(message);
                    self.should_quit = true;
                }
            }
        }
        self.overlay = None;
        Ok(())
    }

    pub(super) async fn submit_add_note(
        &mut self,
        task_id: crate::ids::TaskId,
        display_ref: String,
        body: String,
    ) -> Result<()> {
        if body.trim().is_empty() {
            self.set_warning("note body is required");
            return Ok(());
        }
        let note_id = self.store.add_note_to_task(&task_id, body).await?;
        self.refresh().await?;
        self.set_mutation_success(format!("added note {note_id} to {display_ref}"));
        Ok(())
    }

    pub(super) fn cancel_authoring_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.intake.cancel();
        match self.authoring.cancel_add_task() {
            Some(AddTaskOrigin::EpicChild {
                mut return_search, ..
            }) => {
                self.schedule_search_preview(&mut return_search);
                self.overlay = Some(OverlayState::Search(*return_search));
            }
            Some(AddTaskOrigin::Standalone) | None => {
                self.overlay = None;
            }
        }
    }
}

fn task_label_options(existing: &[String], selected: &[String]) -> Vec<String> {
    let mut options = existing.to_vec();
    options.extend(selected.iter().cloned());
    options.sort();
    options.dedup();
    options
}

fn add_task_natural_intake(title: &str, description: &str) -> String {
    let title = title.trim();
    let description = description.trim();
    match (title.is_empty(), description.is_empty()) {
        (false, false) => format!("Title:\n{title}\n\nDescription:\n{description}"),
        (false, true) => title.to_string(),
        (true, false) => format!("Description:\n{description}"),
        (true, true) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::add_task_natural_intake;

    #[test]
    fn add_task_natural_intake_combines_title_and_description() {
        assert_eq!(
            add_task_natural_intake("Write docs", "Include setup details"),
            "Title:\nWrite docs\n\nDescription:\nInclude setup details"
        );
    }
}

use anyhow::Result;

use crate::config::resolve_blob_dir;
use crate::labels::normalize_label;
use crate::operations::TaskDraft;
use crate::tui::app::{App, Notification};
use crate::tui::app_intake::{IntakeCompletion, IntakePoll, NaturalRetry};
use crate::tui::authoring::{
    ADD_NOTE_TITLE, ADD_TASK_LABELS_TITLE, ADD_TASK_TITLE_PROJECT_TITLE, AddNoteSubmit, AddTaskStep,
};
use crate::tui::natural_add_runtime::task_intake_log_path;
use crate::tui::overlay::{
    AddTaskMode, AddTaskState, LineEdit, MultilineInputState, OverlayRoute, OverlayState,
    PickerState,
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
        self.begin_add_task_title();
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
                OverlayRoute::AddTaskDescription,
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
            available_at: LineEdit::new(context.available_at),
            due_on: LineEdit::new(context.due_on),
            selected_attachment: attachments.len().saturating_sub(1),
            attachments,
            mode: crate::tui::overlay::AddTaskMode::Compose,
            title_error: false,
        })));
    }

    pub(super) fn begin_add_task_step(&mut self) {
        self.begin_add_task_overlay();
    }

    pub(super) fn open_add_task_description_editor(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        self.needs_terminal_clear = true;
        match edit_text_externally(context.description.clone(), "description.md") {
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
            self.authoring.apply_add_task_status(&state.status);
            self.authoring
                .apply_add_task_priority_value(&state.priority);
            self.authoring.apply_add_task_labels(state.labels.clone());
            self.authoring
                .apply_add_task_available_at(state.available_at.text.clone());
            self.authoring
                .apply_add_task_due_on(state.due_on.text.clone());
        }
        captured
    }

    pub(super) fn set_add_task_status(&mut self, status: &str) {
        if let Some(status) = self.authoring.apply_add_task_status(status) {
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.status = status.clone();
            } else {
                self.begin_add_task_step();
            }
            self.set_info(format!("add task status={status}"));
        }
    }

    pub(super) fn set_add_task_priority(&mut self, priority: &str) {
        if let Some(priority) = self.authoring.apply_add_task_priority_value(priority) {
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.priority = priority.clone();
            } else {
                self.begin_add_task_step();
            }
            self.set_info(format!("add task priority={priority}"));
        }
    }

    pub(super) fn open_focused_add_task_control(&mut self) {
        let Some(OverlayState::AddTask(mut state)) = self.overlay.take() else {
            return;
        };
        state.mode = match state.focus {
            AddTaskStep::Project => AddTaskMode::Picker {
                field: state.focus,
                state: PickerState::new(
                    OverlayRoute::AddTaskTitleProject,
                    ADD_TASK_TITLE_PROJECT_TITLE,
                    self.store
                        .project_picker_items(state.selected_project.as_deref()),
                    false,
                ),
            },
            AddTaskStep::Status => AddTaskMode::Picker {
                field: state.focus,
                state: PickerState::new(
                    OverlayRoute::EditStatus,
                    "Add task: status",
                    self.store.status_picker_items(Some(&state.status)),
                    false,
                ),
            },
            AddTaskStep::Priority => AddTaskMode::Picker {
                field: state.focus,
                state: PickerState::new(
                    OverlayRoute::AddTaskTitlePriority,
                    "Add task: priority",
                    self.store.priority_picker_items(&state.priority),
                    false,
                ),
            },
            AddTaskStep::Labels => {
                let OverlayState::TagCombobox(labels) = OverlayState::tag_combobox(
                    OverlayRoute::AddTaskTitleLabels,
                    ADD_TASK_LABELS_TITLE,
                    self.store.labels.clone(),
                    state.labels.clone(),
                ) else {
                    unreachable!();
                };
                AddTaskMode::Labels(labels)
            }
            _ => AddTaskMode::Compose,
        };
        self.overlay = Some(OverlayState::AddTask(state));
    }

    pub(super) fn begin_add_note(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self
            .store
            .selected_task(self.widgets.table.selected())
            .cloned()
        else {
            self.set_info("no selected task for note");
            return;
        };
        let return_to_detail =
            self.detail_context || matches!(self.overlay, Some(OverlayState::Detail { .. }));
        self.authoring.begin_add_note(
            item.task.id.clone(),
            item.display_ref.clone(),
            return_to_detail,
        );
        self.detail_context = return_to_detail;
        self.overlay = Some(OverlayState::blank_multiline_input(
            OverlayRoute::AddNote,
            ADD_NOTE_TITLE,
            "note body:",
        ));
    }

    pub(super) fn begin_add_task_title_labels(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        self.overlay = Some(OverlayState::tag_combobox(
            OverlayRoute::AddTaskTitleLabels,
            ADD_TASK_LABELS_TITLE,
            self.store.labels.clone(),
            context.labels,
        ));
    }

    pub(super) async fn submit_add_task_title_labels(&mut self, labels: Vec<String>) -> Result<()> {
        for label in &labels {
            let label = normalize_label(label);
            if !self.store.labels.contains(&label)
                && let Err(error) = self.store.create_label(label).await
            {
                self.set_error(format!("{error:#}"));
                self.begin_add_task_title_labels();
                return Ok(());
            }
        }
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
            OverlayRoute::AddTaskNatural,
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
            && self.epic_child_authoring.is_none()
        {
            self.intake.start_detached(
                raw,
                &self.store.active_workspace.id,
                project.as_deref(),
                true,
            )?;
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
            Ok(draft) if ready.create_on_success => {
                self.submit_created_task(draft).await?;
                self.authoring.clear_add_task();
            }
            Ok(draft) => {
                if self.authoring.apply_add_task_draft(draft) {
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

    pub(super) async fn submit_created_task(&mut self, draft: TaskDraft) -> Result<()> {
        let attachments = self.authoring.add_task_attachments();
        let current_selected = self.widgets.table.selected();
        if let Some(context) = self.epic_child_authoring.clone() {
            let result = if attachments.is_empty() {
                self.store
                    .create_task_for_epic(draft, current_selected, &context.epic)
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
                        attachments,
                        &context.epic,
                    )
                    .await
            };
            let (message, selected, task_id) = match result {
                Ok(created) => created,
                Err(error) => {
                    self.set_error(format!("{error:#}"));
                    self.begin_add_task_step();
                    return Ok(());
                }
            };
            self.widgets.table.select(
                self.store
                    .tasks
                    .iter()
                    .position(|item| item.task.id == context.epic.epic_id)
                    .or(selected),
            );
            self.selected_detail_child_task_id = Some(task_id);
            self.removed_epic_child = None;
            self.epic_child_authoring = None;
            self.authoring.clear_add_task();
            self.overlay = Some(OverlayState::Detail { scroll: 0 });
            self.detail_context = true;
            self.set_success(message);
            return Ok(());
        }
        let result = if attachments.is_empty() {
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
                    attachments,
                )
                .await
        };
        let (message, selected) = match result {
            Ok(created) => created,
            Err(error) => {
                if crate::tui::store::task_creation_committed(&error) {
                    self.authoring.clear_add_task();
                }
                return Err(error);
            }
        };
        self.widgets.table.select(selected);
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

    pub(super) async fn submit_add_note(&mut self, body: String) -> Result<()> {
        match self.authoring.submit_add_note(body) {
            AddNoteSubmit::Create {
                task_id,
                display_ref,
                body,
                return_to_detail,
            } => {
                let note_id = self.store.add_note_to_task(&task_id, body).await?;
                self.refresh().await?;
                self.restore_detail_overlay(return_to_detail);
                self.set_success(format!("added note {note_id} to {display_ref}"));
            }
            AddNoteSubmit::Blank {
                return_to_detail,
                message,
            } => {
                self.restore_detail_overlay(return_to_detail);
                self.set_warning(message);
            }
            AddNoteSubmit::Inactive { message } => {
                self.set_info(message);
            }
        }
        Ok(())
    }

    pub(super) fn cancel_authoring_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.intake.cancel();
        if let Some(context) = self.epic_child_authoring.take() {
            self.authoring.clear_add_task();
            let mut search = context.search;
            self.schedule_search_preview(&mut search);
            self.overlay = Some(OverlayState::Search(search));
            self.detail_context = true;
            return;
        }
        let return_to_detail = self.authoring.cancel() || self.detail_context;
        self.overlay = None;
        self.conflict_flow.clear();
        self.pending_rename_project = None;
        self.pending_delete_project = None;
        self.pending_delete_attachment = None;
        self.detail_context = false;
        self.restore_detail_overlay(return_to_detail);
    }
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

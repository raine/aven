use anyhow::Result;

use crate::operations::TaskDraft;
use crate::tui::app::{App, NaturalRetry, Notification, PendingTaskIntake, ReadyTaskIntake};
use crate::tui::authoring::{
    ADD_NOTE_TITLE, ADD_TASK_TITLE_PROJECT_TITLE, AddNoteSubmit, AddTaskStep, AddTaskTitleSubmit,
};
use crate::tui::natural_add_runtime::{spawn_add_task_only_natural, task_intake_log_path};
use crate::tui::overlay::{
    AddTaskState, LineEdit, MultilineInputState, OverlayRoute, OverlayState,
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
        self.overlay = Some(OverlayState::AddTask(AddTaskState {
            title: LineEdit::new(context.title),
            description: MultilineInputState::from_value(
                OverlayRoute::AddTaskDescription,
                "Add task: description",
                "",
                context.description,
            ),
            focus: context.step,
            project: context.project,
            status: context.status,
            priority: context.priority,
        }));
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
        self.authoring.capture_add_task_fields(
            state.title.text.clone(),
            state.description.lines.join("\n"),
            state.focus,
        )
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

    pub(super) async fn submit_add_task_from_authoring(&mut self) -> Result<()> {
        match self.authoring.submit_add_task() {
            AddTaskTitleSubmit::ReopenTitle { message } => {
                self.set_warning(message);
                self.begin_add_task_title();
            }
            AddTaskTitleSubmit::Create(draft) => {
                self.submit_created_task(draft).await?;
            }
            AddTaskTitleSubmit::Inactive => {}
        }
        Ok(())
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

    pub(super) fn begin_add_task_title_project(&mut self) {
        let Some(selected) = self.authoring.selected_add_task_project() else {
            return;
        };
        let items = self.store.project_picker_items(selected.as_deref());
        self.open_picker_overlay(
            OverlayRoute::AddTaskTitleProject,
            ADD_TASK_TITLE_PROJECT_TITLE,
            items,
            false,
        );
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
        if self.add_task_only {
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
        spawn_add_task_only_natural(
            raw,
            self.store.active_workspace.id.as_str(),
            self.add_task_db_path.as_deref(),
            project.as_deref(),
        )?;
        self.add_task_only_message = Some("adding task in background".to_string());
        self.should_quit = true;
        Ok(())
    }

    pub(super) async fn submit_add_task_natural(&mut self, value: String) -> Result<()> {
        if self.add_task_only {
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
        let handle = self.store.spawn_task_intake(
            self.add_task_config.agent.task_intake.clone(),
            raw.to_string(),
            project,
        );
        self.notification = Some(Notification::loading(if create_on_success {
            "adding task with LLM"
        } else {
            "parsing task with LLM"
        }));
        self.pending_task_intake = Some(PendingTaskIntake {
            handle,
            retry,
            value: value.clone(),
            create_on_success,
        });
        if create_on_success {
            self.overlay = None;
        } else {
            self.retry_add_task_natural(value, retry);
        }
        Ok(())
    }

    pub(super) async fn poll_pending_task_intake(&mut self) -> Result<bool> {
        if let Some(ready) = self.ready_task_intake.take() {
            self.finish_ready_task_intake(ready).await?;
            return Ok(true);
        }

        let Some(pending) = self
            .pending_task_intake
            .take_if(|pending| pending.handle.is_finished())
        else {
            return Ok(false);
        };
        let ready = ReadyTaskIntake {
            outcome: pending.handle.await?,
            retry: pending.retry,
            value: pending.value,
            create_on_success: pending.create_on_success,
        };
        if ready.outcome.is_err() {
            self.set_error("task intake failed");
        }
        self.ready_task_intake = Some(ready);
        Ok(true)
    }

    async fn finish_ready_task_intake(&mut self, ready: ReadyTaskIntake) -> Result<()> {
        match ready.outcome {
            Ok(draft) if ready.create_on_success => {
                self.submit_created_task(draft).await?;
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

    async fn submit_created_task(&mut self, draft: TaskDraft) -> Result<()> {
        let current_selected = self.widgets.table.selected();
        let (message, selected) = self.store.create_task(draft, current_selected).await?;
        self.widgets.table.select(selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        if selected.is_none() {
            self.restore_selection_after_mutation();
        }
        self.set_success(message.clone());
        if self.add_task_only {
            self.add_task_only_message = Some(message);
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
        let return_to_detail = self.authoring.cancel() || self.detail_context;
        self.overlay = None;
        self.conflict_flow.clear();
        self.pending_rename_project = None;
        self.pending_delete_project = None;
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

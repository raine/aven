use anyhow::Result;

use crate::tui::app::App;
use crate::tui::overlay::{MultilineInputState, OverlayRoute, OverlayState, TextInputState};

pub(crate) const EDIT_STATUS_TITLE: &str = "Edit task: status";
pub(crate) const EDIT_TITLE_TITLE: &str = "Edit title";
pub(crate) const EDIT_DESCRIPTION_TITLE: &str = "Edit description";
pub(crate) const EDIT_PROJECT_TITLE: &str = "Edit project";
pub(crate) const EDIT_PRIORITY_TITLE: &str = "Edit task: priority";
pub(crate) const EDIT_LABELS_TITLE: &str = "Edit task: labels";

impl App {
    pub(super) async fn update_status(&mut self, status: &'static str) -> Result<()> {
        if let Some(result) = self
            .store
            .update_status(self.widgets.table.selected(), status)
            .await?
        {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn set_exact_priority(&mut self, priority: &'static str) -> Result<()> {
        if let Some(result) = self
            .store
            .set_exact_priority(self.widgets.table.selected(), priority)
            .await?
        {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn update_priority(&mut self, reverse: bool) -> Result<()> {
        if let Some(result) = self
            .store
            .update_priority(self.widgets.table.selected(), reverse)
            .await?
        {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn update_deleted(&mut self, deleted: bool) -> Result<()> {
        if let Some(result) = self
            .store
            .update_deleted(self.widgets.table.selected(), deleted)
            .await?
        {
            self.apply_mutation_result(result);
        } else {
            self.set_info("no selected task to edit");
        }
        Ok(())
    }

    pub(super) async fn undo_last(&mut self) -> Result<()> {
        match self.store.undo_last(self.widgets.table.selected()).await? {
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
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let selected = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .status
            .as_str();
        let items = self.store.status_picker_items(Some(selected));
        self.open_picker_overlay(OverlayRoute::EditStatus, EDIT_STATUS_TITLE, items, false);
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
        self.overlay = Some(OverlayState::TextInput(TextInputState::new(
            OverlayRoute::EditTitle,
            EDIT_TITLE_TITLE,
            "",
            input,
        )));
    }

    fn open_edit_description_overlay(&mut self, value: String) {
        self.overlay = Some(OverlayState::MultilineInput(
            MultilineInputState::from_value(
                OverlayRoute::EditDescription,
                EDIT_DESCRIPTION_TITLE,
                "",
                value,
            ),
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
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let priority = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .priority
            .as_str();
        let items = self.store.priority_picker_items(priority);
        self.open_picker_overlay(
            OverlayRoute::EditPriority,
            EDIT_PRIORITY_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_edit_labels(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let labels = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .labels
            .clone();
        let mut items = self.store.label_picker_items();
        for picker_item in &mut items {
            picker_item.selected = labels.contains(&picker_item.value);
        }
        self.open_picker_overlay(OverlayRoute::EditLabels, EDIT_LABELS_TITLE, items, true);
    }

    pub(super) async fn submit_edit_status(&mut self, status: String) -> Result<()> {
        let result = self
            .store
            .update_status(self.widgets.table.selected(), &status)
            .await;
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
        let result = self
            .store
            .update_project(self.widgets.table.selected(), project)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_project());
        Ok(())
    }

    pub(super) async fn submit_edit_priority(&mut self, priority: String) -> Result<()> {
        let result = self
            .store
            .set_exact_priority(self.widgets.table.selected(), &priority)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_priority());
        Ok(())
    }

    pub(super) async fn submit_edit_labels(&mut self, labels: Vec<String>) -> Result<()> {
        let result = self
            .store
            .update_labels(self.widgets.table.selected(), labels)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_labels());
        Ok(())
    }
}

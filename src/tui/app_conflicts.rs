use anyhow::Result;

use crate::tui::app::App;
use crate::tui::conflict_flow::{ConflictResolutionChoice, truncate_value_preview};
use crate::tui::overlay::{
    ConfirmIntent, MultilineIntent, OverlayState, PickerIntent, PickerItem, TextIntent,
    TextPanelState,
};
use crate::tui::store::deleted_picker_items;
use crate::tui::store::{ConflictTarget, TaskQuery};

pub(crate) const CONFLICT_FIELD_TITLE: &str = "Conflict: field";
pub(crate) const CONFLICT_CONFIRM_LOCAL_TITLE: &str = "Resolve conflict: local";
pub(crate) const CONFLICT_CONFIRM_REMOTE_TITLE: &str = "Resolve conflict: remote";
pub(crate) const CONFLICT_MANUAL_TITLE: &str = "Resolve conflict: manual";
pub(crate) const CONFLICT_DETAILS_TITLE: &str = "Conflict details";

impl App {
    pub(super) async fn open_conflict_list(&mut self) -> Result<()> {
        let selected = self.store.show_view(TaskQuery::Conflicts).await?;
        self.apply_filter_selection(selected);
        let count = self
            .store
            .tasks
            .iter()
            .filter(|task| task.has_conflict)
            .count();
        let message = if count == 0 {
            "no unresolved conflicts".to_string()
        } else {
            format!("showing {count} conflicted tasks")
        };
        self.set_info(message);
        Ok(())
    }

    async fn conflict_targets_for_selected(&mut self) -> Result<Option<Vec<ConflictTarget>>> {
        self.store.conflict_targets(self.list.selected_task()).await
    }

    async fn load_conflict_targets_for_resolution(
        &mut self,
    ) -> Result<Option<Vec<ConflictTarget>>> {
        let Some(targets) = self.conflict_targets_for_selected().await? else {
            self.set_info("no selected task for conflict resolution");
            return Ok(None);
        };
        if targets.is_empty() {
            self.set_info("selected task has no unresolved conflicts");
            return Ok(None);
        }
        Ok(Some(targets))
    }

    pub(super) async fn show_conflict_details_for(
        &mut self,
        item: &crate::query::TaskListItem,
    ) -> Result<()> {
        let targets = self.store.conflict_targets_for(item).await?;
        if targets.is_empty() {
            self.set_info(format!("{} has no unresolved conflicts", item.display_ref));
            return Ok(());
        }
        let mut lines = Vec::new();
        for target in &targets {
            lines.push(format!("field={}", target.field));
            lines.push(format!(
                "local {}: {}",
                target.variant_a, target.local_value
            ));
            lines.push(format!(
                "remote {}: {}",
                target.variant_b, target.remote_value
            ));
            lines.push(String::new());
        }
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        self.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            CONFLICT_DETAILS_TITLE,
            lines,
        )));
        Ok(())
    }

    pub(super) async fn show_conflict_details(&mut self) -> Result<()> {
        let Some(item) = self.store.selected_task(self.list.selected_task()).cloned() else {
            self.set_info("no selected task for conflicts");
            return Ok(());
        };
        self.show_conflict_details_for(&item).await
    }

    pub(super) fn move_to_conflict(&mut self, delta: isize) {
        let current = self.list.selected_task();
        let Some(next) = self.store.next_conflict_index(current, delta) else {
            self.set_info("no conflicts in current list");
            return;
        };
        if current == Some(next) {
            self.set_info("selected only conflict");
            return;
        }
        self.list.select_task(Some(next));
        self.list.focus_tasks();
    }

    pub(super) fn begin_conflict_resolution_for(
        &mut self,
        choice: ConflictResolutionChoice,
        targets: Vec<ConflictTarget>,
    ) {
        if targets.is_empty() {
            self.set_info("selected task has no unresolved conflicts");
            return;
        }
        if targets.len() == 1 {
            self.open_conflict_confirm(choice, targets.into_iter().next().unwrap());
        } else {
            self.open_conflict_field_picker(
                PickerIntent::PickConflictVariant {
                    choice,
                    targets: targets.clone(),
                },
                &targets,
            );
        }
    }

    pub(super) async fn begin_conflict_resolution(
        &mut self,
        choice: ConflictResolutionChoice,
    ) -> Result<()> {
        let Some(targets) = self.load_conflict_targets_for_resolution().await? else {
            return Ok(());
        };
        self.begin_conflict_resolution_for(choice, targets);
        Ok(())
    }

    pub(super) fn begin_manual_conflict_merge_for(&mut self, targets: Vec<ConflictTarget>) {
        if targets.is_empty() {
            self.set_info("selected task has no unresolved conflicts");
            return;
        }
        if targets.len() == 1 {
            self.open_manual_conflict_editor(targets.into_iter().next().unwrap());
        } else {
            self.open_conflict_field_picker(
                PickerIntent::PickConflictManual {
                    targets: targets.clone(),
                },
                &targets,
            );
        }
    }

    pub(super) async fn begin_manual_conflict_merge(&mut self) -> Result<()> {
        let Some(targets) = self.load_conflict_targets_for_resolution().await? else {
            return Ok(());
        };
        self.begin_manual_conflict_merge_for(targets);
        Ok(())
    }

    fn open_conflict_field_picker(&mut self, intent: PickerIntent, targets: &[ConflictTarget]) {
        let items = targets
            .iter()
            .map(|target| PickerItem {
                label: target.field.clone(),
                value: target.field.clone(),
                selected: false,
            })
            .collect();
        self.open_picker_overlay(intent, CONFLICT_FIELD_TITLE, items, false);
    }

    pub(super) async fn submit_conflict_field_picker(
        &mut self,
        intent: PickerIntent,
        values: Vec<String>,
    ) -> Result<()> {
        let Some(field) = values.first().filter(|value| !value.is_empty()) else {
            self.set_warning("no conflict field selected");
            return Ok(());
        };
        let (choice, targets) = match intent {
            PickerIntent::PickConflictVariant { choice, targets } => (Some(choice), targets),
            PickerIntent::PickConflictManual { targets } => (None, targets),
            _ => return Ok(()),
        };
        let Some(target) = targets.into_iter().find(|target| target.field == *field) else {
            self.set_warning(format!("no conflict for field={field}"));
            return Ok(());
        };
        if let Some(choice) = choice {
            self.open_conflict_confirm(choice, target);
        } else {
            self.open_manual_conflict_editor(target);
        }
        Ok(())
    }

    fn open_conflict_confirm(&mut self, choice: ConflictResolutionChoice, target: ConflictTarget) {
        let value = match choice {
            ConflictResolutionChoice::Local => target.local_value.clone(),
            ConflictResolutionChoice::Remote => target.remote_value.clone(),
        };
        let title = match choice {
            ConflictResolutionChoice::Local => CONFLICT_CONFIRM_LOCAL_TITLE,
            ConflictResolutionChoice::Remote => CONFLICT_CONFIRM_REMOTE_TITLE,
        };
        let prompt = format!(
            "Resolve field={} with {}?",
            target.field,
            truncate_value_preview(&value, 60)
        );
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::ResolveConflict { target, value },
            title,
            prompt,
        ));
    }

    pub(super) async fn submit_confirmed_conflict_resolution(
        &mut self,
        target: ConflictTarget,
        value: String,
    ) -> Result<()> {
        match self.store.resolve_conflict_value(target, value).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => self.set_error(format!("{error:#}")),
        }
        Ok(())
    }

    fn open_manual_conflict_editor(&mut self, target: ConflictTarget) {
        match target.field.as_str() {
            "description" => {
                self.overlay = Some(OverlayState::multiline_input(
                    MultilineIntent::ResolveConflictManually {
                        target: target.clone(),
                    },
                    CONFLICT_MANUAL_TITLE,
                    format!("manual value for field={}:", target.field),
                    target.local_value.clone(),
                ));
            }
            "title" => {
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::ResolveConflictManually {
                        target: target.clone(),
                    },
                    CONFLICT_MANUAL_TITLE,
                    format!("manual value for field={}:", target.field),
                    target.local_value.clone(),
                ));
            }
            "status" => {
                let items = self
                    .store
                    .status_picker_items(Some(target.local_value.as_str()));
                self.open_picker_overlay(
                    PickerIntent::ResolveConflictManually {
                        target: target.clone(),
                    },
                    CONFLICT_MANUAL_TITLE,
                    items,
                    false,
                );
            }
            "priority" => {
                let items = self
                    .store
                    .priority_picker_items(target.local_value.as_str());
                self.open_picker_overlay(
                    PickerIntent::ResolveConflictManually {
                        target: target.clone(),
                    },
                    CONFLICT_MANUAL_TITLE,
                    items,
                    false,
                );
            }
            "project" => {
                let items = self
                    .store
                    .existing_project_picker_items(target.local_value.as_str());
                self.open_picker_overlay(
                    PickerIntent::ResolveConflictManually {
                        target: target.clone(),
                    },
                    CONFLICT_MANUAL_TITLE,
                    items,
                    false,
                );
            }
            "deleted" => {
                let items = deleted_picker_items(&target.local_value);
                self.open_picker_overlay(
                    PickerIntent::ResolveConflictManually {
                        target: target.clone(),
                    },
                    CONFLICT_MANUAL_TITLE,
                    items,
                    false,
                );
            }
            _ => {
                self.overlay = None;
                self.set_warning(format!(
                    "manual merge is not supported for field={}",
                    target.field
                ));
            }
        }
    }

    pub(super) async fn submit_manual_conflict_value(
        &mut self,
        target: ConflictTarget,
        value: String,
    ) -> Result<()> {
        match self
            .store
            .resolve_conflict_value(target.clone(), value.clone())
            .await
        {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                let mut retry_target = target;
                retry_target.local_value = value;
                self.open_manual_conflict_editor(retry_target);
            }
        }
        Ok(())
    }
}

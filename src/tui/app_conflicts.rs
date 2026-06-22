use anyhow::Result;

use crate::tui::app::{App, Focus};
use crate::tui::conflict_flow::{
    ConflictResolutionChoice, ConflictSubmit, ConflictTransition, truncate_value_preview,
};
use crate::tui::overlay::{
    ConfirmState, MultilineInputState, OverlayRoute, OverlayState, PickerItem, TextInputState,
    TextPanelState,
};
use crate::tui::store::deleted_picker_items;
use crate::tui::store::{ConflictTarget, SidebarTarget};

pub(crate) const CONFLICT_FIELD_TITLE: &str = "Conflict: field";
pub(crate) const CONFLICT_CONFIRM_LOCAL_TITLE: &str = "Resolve conflict: local";
pub(crate) const CONFLICT_CONFIRM_REMOTE_TITLE: &str = "Resolve conflict: remote";
pub(crate) const CONFLICT_MANUAL_TITLE: &str = "Resolve conflict: manual";
pub(crate) const CONFLICT_DETAILS_TITLE: &str = "Conflict details";

impl App {
    pub(super) async fn open_conflict_list(&mut self) -> Result<()> {
        let selected = self.store.show_view(SidebarTarget::Conflicts).await?;
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
        self.store
            .conflict_targets(self.widgets.table.selected())
            .await
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

    pub(super) async fn show_conflict_details(&mut self) -> Result<()> {
        let Some(targets) = self.conflict_targets_for_selected().await? else {
            self.set_info("no selected task for conflicts");
            return Ok(());
        };
        if targets.is_empty() {
            let display_ref = self
                .store
                .selected_task(self.widgets.table.selected())
                .map(|item| item.display_ref.clone())
                .unwrap_or_else(|| "task".to_string());
            self.set_info(format!("{display_ref} has no unresolved conflicts"));
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

    pub(super) fn move_to_conflict(&mut self, delta: isize) {
        let current = self.widgets.table.selected();
        let Some(next) = self.store.next_conflict_index(current, delta) else {
            self.set_info("no conflicts in current list");
            return;
        };
        if current == Some(next) {
            self.set_info("selected only conflict");
            return;
        }
        self.widgets.table.select(Some(next));
        self.focus = Focus::Tasks;
        let message = if delta > 0 {
            "selected next conflict"
        } else {
            "selected previous conflict"
        };
        self.set_info(message);
    }

    fn apply_conflict_transition(&mut self, transition: ConflictTransition) {
        match transition {
            ConflictTransition::PickField { targets } => self.open_conflict_field_picker(&targets),
            ConflictTransition::Confirm { choice, target } => {
                self.open_conflict_confirm(choice, target)
            }
            ConflictTransition::EditManual { target } => self.open_manual_conflict_editor(target),
            ConflictTransition::Message(message) => self.set_warning(message),
        }
    }

    pub(super) async fn begin_conflict_resolution(
        &mut self,
        choice: ConflictResolutionChoice,
    ) -> Result<()> {
        let Some(targets) = self.load_conflict_targets_for_resolution().await? else {
            return Ok(());
        };
        let transition = self.conflict_flow.begin_resolution(choice, targets);
        self.apply_conflict_transition(transition);
        Ok(())
    }

    pub(super) async fn begin_manual_conflict_merge(&mut self) -> Result<()> {
        let Some(targets) = self.load_conflict_targets_for_resolution().await? else {
            return Ok(());
        };
        let transition = self.conflict_flow.begin_manual(targets);
        self.apply_conflict_transition(transition);
        Ok(())
    }

    fn open_conflict_field_picker(&mut self, targets: &[ConflictTarget]) {
        let items = targets
            .iter()
            .map(|target| PickerItem {
                label: target.field.clone(),
                value: target.field.clone(),
                selected: false,
            })
            .collect();
        self.open_picker_overlay(
            OverlayRoute::ConflictField,
            CONFLICT_FIELD_TITLE,
            items,
            false,
        );
    }

    pub(super) async fn submit_conflict_field_picker(&mut self, values: Vec<String>) -> Result<()> {
        let transition = self.conflict_flow.submit_field(values);
        self.apply_conflict_transition(transition);
        Ok(())
    }

    fn open_conflict_confirm(&mut self, choice: ConflictResolutionChoice, target: ConflictTarget) {
        let value = match choice {
            ConflictResolutionChoice::Local => target.local_value.as_str(),
            ConflictResolutionChoice::Remote => target.remote_value.as_str(),
        };
        let title = match choice {
            ConflictResolutionChoice::Local => CONFLICT_CONFIRM_LOCAL_TITLE,
            ConflictResolutionChoice::Remote => CONFLICT_CONFIRM_REMOTE_TITLE,
        };
        self.overlay = Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::ConflictConfirm,
            title: title.to_string(),
            prompt: format!(
                "Resolve field={} with {}?",
                target.field,
                truncate_value_preview(value, 60)
            ),
        }));
    }

    pub(super) async fn submit_confirmed_conflict_resolution(&mut self) -> Result<()> {
        match self.conflict_flow.submit_confirmed_variant() {
            ConflictSubmit::Resolve { target, value } => {
                match self.store.resolve_conflict_value(target, value).await {
                    Ok(result) => self.apply_mutation_result(result),
                    Err(error) => self.set_error(format!("{error:#}")),
                }
            }
            ConflictSubmit::Inactive { message } => self.set_warning(message),
        }
        Ok(())
    }

    fn open_manual_conflict_editor(&mut self, target: ConflictTarget) {
        match target.field.as_str() {
            "description" => {
                self.overlay = Some(OverlayState::MultilineInput(
                    MultilineInputState::from_value(
                        OverlayRoute::ConflictManual,
                        CONFLICT_MANUAL_TITLE,
                        format!("manual value for field={}:", target.field),
                        target.local_value.clone(),
                    ),
                ));
            }
            "title" => {
                self.overlay = Some(OverlayState::TextInput(TextInputState::new(
                    OverlayRoute::ConflictManual,
                    CONFLICT_MANUAL_TITLE,
                    format!("manual value for field={}:", target.field),
                    target.local_value.clone(),
                )));
            }
            "status" => {
                let items = self
                    .store
                    .status_picker_items(Some(target.local_value.as_str()));
                self.open_picker_overlay(
                    OverlayRoute::ConflictManual,
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
                    OverlayRoute::ConflictManual,
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
                    OverlayRoute::ConflictManual,
                    CONFLICT_MANUAL_TITLE,
                    items,
                    false,
                );
            }
            "deleted" => {
                let items = deleted_picker_items(&target.local_value);
                self.open_picker_overlay(
                    OverlayRoute::ConflictManual,
                    CONFLICT_MANUAL_TITLE,
                    items,
                    false,
                );
            }
            _ => {
                self.conflict_flow.clear();
                self.overlay = None;
                self.set_warning(format!(
                    "manual merge is not supported for field={}",
                    target.field
                ));
            }
        }
    }

    pub(super) async fn submit_manual_conflict_value(&mut self, value: String) -> Result<()> {
        match self.conflict_flow.submit_manual_value(value) {
            ConflictSubmit::Resolve { target, value } => {
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
                        let transition = self.conflict_flow.retry_manual_edit(retry_target);
                        self.apply_conflict_transition(transition);
                    }
                }
            }
            ConflictSubmit::Inactive { message } => self.set_warning(message),
        }
        Ok(())
    }
}

use anyhow::Result;

use crate::projects::normalize_key;
use crate::tui::app::App;
use crate::tui::overlay::{OverlayState, PickerIntent, TextIntent};
use crate::workspaces::Workspace;

pub(crate) const ADD_WORKSPACE_TITLE: &str = "Create workspace";
pub(crate) const RENAME_WORKSPACE_TITLE: &str = "Rename workspace";
pub(crate) const DELETE_WORKSPACE_TITLE: &str = "Delete workspace";

fn workspace_delete_prompt(workspace: &Workspace, warnings: &[String]) -> String {
    let mut lines = warnings.to_vec();
    lines.push(format!(
        "Type {} to delete workspace {} on every device:",
        workspace.key, workspace.name
    ));
    lines.join("\n")
}

fn validate_workspace_name(value: &str) -> std::result::Result<String, &'static str> {
    let name = value.trim();
    if name.is_empty() {
        return Err("workspace name is required");
    }
    if normalize_key(name).is_empty() {
        return Err("workspace name must contain an ASCII letter or number");
    }
    Ok(name.to_string())
}

impl App {
    pub(super) fn begin_add_workspace(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::blank_text_input(
            TextIntent::AddWorkspace,
            ADD_WORKSPACE_TITLE,
            "workspace name:",
        ));
    }

    pub(super) fn begin_rename_workspace(&mut self) {
        self.pending_shortcut.clear();
        let items = self.store.workspace_rename_picker_items();
        self.open_picker_overlay(
            PickerIntent::RenameWorkspace,
            RENAME_WORKSPACE_TITLE,
            items,
            false,
        );
    }

    pub(super) fn submit_rename_workspace_picker(&mut self, values: Vec<String>) {
        let Some(workspace) = self.require_picker_value(values, "no matching workspace") else {
            self.begin_rename_workspace();
            return;
        };
        let name = self
            .store
            .workspaces
            .iter()
            .find(|candidate| candidate.key == workspace)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_else(|| workspace.clone());
        self.overlay = Some(OverlayState::text_input(
            TextIntent::RenameWorkspace { workspace },
            RENAME_WORKSPACE_TITLE,
            "new workspace name:",
            name,
        ));
    }

    pub(super) async fn submit_add_workspace(&mut self, value: String) -> Result<()> {
        let name = match validate_workspace_name(&value) {
            Ok(name) => name,
            Err(message) => {
                self.set_warning(message);
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::AddWorkspace,
                    ADD_WORKSPACE_TITLE,
                    "workspace name:",
                    value,
                ));
                return Ok(());
            }
        };
        match self.store.create_workspace(name).await {
            Ok(message) => self.set_success(message),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::AddWorkspace,
                    ADD_WORKSPACE_TITLE,
                    "workspace name:",
                    value,
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn submit_rename_workspace(
        &mut self,
        workspace: String,
        value: String,
    ) -> Result<()> {
        let name = match validate_workspace_name(&value) {
            Ok(name) => name,
            Err(message) => {
                self.set_warning(message);
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::RenameWorkspace { workspace },
                    RENAME_WORKSPACE_TITLE,
                    "new workspace name:",
                    value,
                ));
                return Ok(());
            }
        };
        match self.store.rename_workspace(workspace.clone(), name).await {
            Ok(message) => self.set_success(message),
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.overlay = Some(OverlayState::text_input(
                    TextIntent::RenameWorkspace { workspace },
                    RENAME_WORKSPACE_TITLE,
                    "new workspace name:",
                    value,
                ));
            }
        }
        Ok(())
    }

    pub(super) fn begin_delete_workspace(&mut self) {
        self.pending_shortcut.clear();
        let items = self.store.workspace_delete_picker_items();
        if items.is_empty() {
            self.set_warning("only the default workspace exists, and it cannot be deleted");
            return;
        }
        self.open_picker_overlay(
            PickerIntent::DeleteWorkspace,
            DELETE_WORKSPACE_TITLE,
            items,
            false,
        );
    }

    pub(super) async fn submit_delete_workspace_picker(
        &mut self,
        values: Vec<String>,
    ) -> Result<()> {
        let Some(key) = self.require_picker_value(values, "no matching workspace") else {
            self.begin_delete_workspace();
            return Ok(());
        };
        let Some(workspace) = self
            .store
            .workspaces
            .iter()
            .find(|candidate| candidate.key == key)
            .cloned()
        else {
            self.set_warning("workspace no longer exists");
            return Ok(());
        };
        let warnings = self.store.workspace_deletion_warnings(&workspace).await?;
        self.overlay = Some(OverlayState::text_input(
            TextIntent::ConfirmDeleteWorkspace {
                workspace: workspace.clone(),
                warnings: warnings.clone(),
            },
            DELETE_WORKSPACE_TITLE,
            workspace_delete_prompt(&workspace, &warnings),
            String::new(),
        ));
        Ok(())
    }

    pub(super) async fn submit_delete_workspace_name(
        &mut self,
        workspace: Workspace,
        warnings: Vec<String>,
        value: String,
    ) -> Result<()> {
        if value.trim() != workspace.key {
            self.set_warning("workspace key does not match");
            let prompt = workspace_delete_prompt(&workspace, &warnings);
            self.overlay = Some(OverlayState::text_input(
                TextIntent::ConfirmDeleteWorkspace {
                    workspace,
                    warnings,
                },
                DELETE_WORKSPACE_TITLE,
                prompt,
                value,
            ));
            return Ok(());
        }
        match self.store.delete_workspace(&workspace).await {
            Ok(message) => {
                let message = if self.report_deleted_active_workspace() {
                    format!("{message}; switched to default")
                } else {
                    message
                };
                self.set_success(message);
            }
            Err(error) => self.set_error(format!("{error:#}")),
        }
        Ok(())
    }

    /// Reports a switch to the default workspace after a refresh found the
    /// active workspace deleted, here or on another device.
    pub(super) fn report_deleted_active_workspace(&mut self) -> bool {
        let Some(name) = self.store.take_deleted_active_workspace() else {
            return false;
        };
        self.list.clear_last_change();
        self.clear_navigation_history();
        self.detail.close();
        self.prune_task_marks();
        self.set_warning(format!("workspace {name} was deleted; switched to default"));
        true
    }

    pub(super) fn workspace_cancellation_message(&self) -> Option<&'static str> {
        match self.overlay.as_ref() {
            Some(OverlayState::TextInput(state)) => match &state.intent {
                TextIntent::AddWorkspace => Some("workspace creation canceled"),
                TextIntent::RenameWorkspace { .. } => Some("workspace rename canceled"),
                TextIntent::ConfirmDeleteWorkspace { .. } => Some("workspace deletion canceled"),
                _ => None,
            },
            Some(OverlayState::Picker(state)) => match state.intent {
                PickerIntent::RenameWorkspace => Some("workspace rename canceled"),
                PickerIntent::DeleteWorkspace => Some("workspace deletion canceled"),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_names_are_trimmed_and_require_a_key() {
        assert_eq!(
            validate_workspace_name("  Client Work  "),
            Ok("Client Work".to_string())
        );
        assert_eq!(
            validate_workspace_name("   "),
            Err("workspace name is required")
        );
        assert_eq!(
            validate_workspace_name("---"),
            Err("workspace name must contain an ASCII letter or number")
        );
    }
}

use anyhow::Result;

use crate::projects::normalize_key;
use crate::tui::app::App;
use crate::tui::overlay::{OverlayState, PickerIntent, TextIntent};

pub(crate) const ADD_WORKSPACE_TITLE: &str = "Create workspace";
pub(crate) const RENAME_WORKSPACE_TITLE: &str = "Rename workspace";

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

    pub(super) fn workspace_cancellation_message(&self) -> Option<&'static str> {
        match self.overlay.as_ref() {
            Some(OverlayState::TextInput(state)) => match &state.intent {
                TextIntent::AddWorkspace => Some("workspace creation canceled"),
                TextIntent::RenameWorkspace { .. } => Some("workspace rename canceled"),
                _ => None,
            },
            Some(OverlayState::Picker(state)) if state.intent == PickerIntent::RenameWorkspace => {
                Some("workspace rename canceled")
            }
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

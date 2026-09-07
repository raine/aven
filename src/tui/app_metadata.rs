use crate::tui::app::App;
use crate::tui::overlay::metadata::{MetadataEntry, MetadataFocus, MetadataState, MetadataTarget};
use crate::tui::overlay::{LineEdit, OverlayState, TextBuffer};
use crate::tui::task_selection::TaskSelection;
use anyhow::Result;
use aven_core::metadata::TaskMetadataValue;

impl App {
    pub(super) async fn begin_edit_metadata(&mut self) -> Result<()> {
        let Some(selection) = self.resolve_task_selection() else {
            return Ok(());
        };
        if !selection.is_single() {
            self.set_warning("Custom metadata edits require one task");
            return Ok(());
        }
        self.begin_edit_metadata_for(selection).await
    }

    pub(super) async fn begin_edit_metadata_for(&mut self, selection: TaskSelection) -> Result<()> {
        let values = self
            .store
            .metadata_values(selection.single_id().expect("single metadata target"))
            .await?;
        let target = MetadataTarget {
            workspace_id: self.store.active_workspace.id.clone(),
            selection,
        };
        self.open_metadata(target, values).await
    }

    async fn open_metadata(
        &mut self,
        target: MetadataTarget,
        mut values: Vec<TaskMetadataValue>,
    ) -> Result<()> {
        let fields = self.store.metadata_fields().await?;
        let entries = fields
            .into_iter()
            .map(|field| {
                let value = values
                    .iter()
                    .position(|value| value.field_id == field.id)
                    .map(|index| values.swap_remove(index).value);
                MetadataEntry { field, value }
            })
            .collect();
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Metadata(Box::new(MetadataState {
            target,
            entries,
            filter: LineEdit::blank(),
            selected: 0,
            editor: None,
            error: None,
        })));
        Ok(())
    }

    pub(super) fn open_metadata_external_editor(&mut self, mut state: Box<MetadataState>) {
        if let Some(editor) = &mut state.editor
            && !editor.discard
        {
            self.prepare_terminal_transition();
            let result = crate::tui::platform::edit_text_externally(
                editor.input.lines.join("\n"),
                "metadata.txt",
                self.terminal_mouse_capture,
            );
            match result {
                Ok(value) => {
                    editor.input =
                        TextBuffer::from_value_with_baseline(value, editor.input.baseline_value());
                    editor.focus = if editor.is_multiline() {
                        MetadataFocus::Save
                    } else {
                        MetadataFocus::Input
                    };
                    state.error = None;
                }
                Err(error) => state.error = Some(format!("Editor failed: {error:#}")),
            }
        }
        self.overlay = Some(OverlayState::Metadata(state));
    }

    pub(super) async fn save_metadata(
        &mut self,
        mut state: Box<MetadataState>,
        remove: bool,
    ) -> Result<()> {
        let Some(editor) = &state.editor else {
            return Ok(());
        };
        if !remove && !editor.can_save() {
            self.overlay = Some(OverlayState::Metadata(state));
            return Ok(());
        }
        let value = (!remove).then(|| editor.input.lines.join("\n"));
        let entry = &state.entries[state.selected];
        let field = entry.field.clone();
        let result = if state.target.workspace_id != self.store.active_workspace.id {
            Err(anyhow::anyhow!(
                "Metadata workspace changed; reopen the task"
            ))
        } else {
            self.store
                .mutate_metadata(&state.target.selection, &field, value.clone())
                .await
                .map(|result| {
                    self.apply_mutation_result(result);
                })
        };
        match result {
            Ok(()) => {
                state.entries[state.selected].value = value;
                state.editor = None;
                state.error = None;
                state.normalize_selection();
            }
            Err(error) if crate::tui::store::mutation_committed(&error) => {
                return Err(error);
            }
            Err(error) => {
                state.error = Some(metadata_error(&error));
                if error.to_string() == "error metadata-field-changed"
                    && let Ok(fields) = self.store.metadata_fields().await
                    && let Some(renamed) = fields
                        .into_iter()
                        .find(|candidate| candidate.id == field.id)
                {
                    state.error = Some(format!(
                        "Field is named {}. Save again to confirm.",
                        renamed.key
                    ));
                    state.entries[state.selected].field = renamed;
                }
            }
        }
        self.overlay = Some(OverlayState::Metadata(state));
        Ok(())
    }
}

fn metadata_error(error: &anyhow::Error) -> String {
    let message = error.to_string();
    if message.starts_with("error metadata-value-too-large") {
        "Value exceeds 4096 UTF-8 bytes. Shorten it.".to_string()
    } else if message.starts_with("error metadata-values-too-large") {
        "Task values exceed 32768 bytes. Shorten or remove a value.".to_string()
    } else if message.starts_with("error too-many-metadata-values") {
        "A task supports 128 values. Remove a value first.".to_string()
    } else {
        format!("Save failed: {error:#}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::TaskDraft;
    use aven_core::metadata::TaskMetadataInput;

    #[tokio::test]
    async fn entries_follow_field_ids_and_definition_order_across_renames() {
        let dir = tempfile::tempdir().unwrap();
        let database = aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut app = App::new_for_tests(database.clone()).await.unwrap();
        app.store
            .create_task(
                TaskDraft {
                    title: "Metadata construction".to_string(),
                    description: String::new(),
                    project: None,
                    status: "inbox".to_string(),
                    priority: "none".to_string(),
                    source: crate::choices::TaskSource::Unknown,
                    labels: Vec::new(),
                    metadata: ["alpha", "empty", "unset"]
                        .into_iter()
                        .map(|key| TaskMetadataInput {
                            expected_field_id: None,
                            key: key.to_string(),
                            value: String::new(),
                        })
                        .collect(),
                    available_at: None,
                    due_on: None,
                    is_epic: false,
                },
                None,
            )
            .await
            .unwrap();
        let selection = TaskSelection::resolve_single(&app.store.tasks, Some(0)).unwrap();
        let mut values = app
            .store
            .metadata_values(selection.single_id().unwrap())
            .await
            .unwrap();
        values.retain(|value| value.key != "unset");
        let exact = "  opaque\r\né\n";
        values
            .iter_mut()
            .find(|value| value.key == "alpha")
            .unwrap()
            .value = exact.to_string();
        values.reverse();
        database
            .rename_metadata_field(&app.store.active_workspace, "alpha", "zulu")
            .await
            .unwrap();
        let target = MetadataTarget {
            workspace_id: app.store.active_workspace.id.clone(),
            selection,
        };
        app.open_metadata(target.clone(), values).await.unwrap();
        let Some(OverlayState::Metadata(state)) = &app.overlay else {
            panic!("expected metadata overlay")
        };
        assert_eq!(state.target, target);
        assert_eq!(
            state
                .entries
                .iter()
                .map(|entry| entry.field.key.as_str())
                .collect::<Vec<_>>(),
            vec!["empty", "unset", "zulu"]
        );
        assert_eq!(state.entries[0].value.as_deref(), Some(""));
        assert_eq!(state.entries[1].value, None);
        assert_eq!(state.entries[2].value.as_deref(), Some(exact));
    }
}

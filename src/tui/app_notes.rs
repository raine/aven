use anyhow::Result;

use crate::tui::app::{App, DetailTargetId};
use crate::tui::overlay::{ConfirmIntent, MultilineInputState, MultilineIntent, OverlayState};
use crate::tui::platform::edit_text_externally;

pub(crate) const EDIT_NOTE_TITLE: &str = "Edit note";
pub(crate) const DELETE_NOTE_TITLE: &str = "Delete note";

impl App {
    pub(super) fn open_note_external_editor(&mut self, state: MultilineInputState) {
        self.prepare_terminal_transition();
        let intent = state.intent.clone();
        let title = state.title.clone();
        let prompt = state.prompt.clone();
        let baseline = state.baseline_value();
        match edit_text_externally(
            state.lines.join("\n"),
            "note.md",
            self.terminal_mouse_capture,
        ) {
            Ok(value) => {
                self.overlay = Some(OverlayState::multiline_input_with_baseline(
                    intent, title, prompt, value, baseline,
                ));
            }
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.overlay = Some(OverlayState::MultilineInput(state));
            }
        }
    }

    pub(super) fn begin_edit_note(&mut self, note_id: &str, scroll: u16) {
        let Some((task_id, display_ref, body)) = self
            .store
            .selected_task(self.list.selected_task())
            .and_then(|item| {
                item.notes
                    .iter()
                    .find(|note| note.id == note_id)
                    .map(|note| {
                        (
                            item.task.id.clone(),
                            item.display_ref.clone(),
                            note.body.clone(),
                        )
                    })
            })
        else {
            self.set_warning("note is unavailable");
            return;
        };
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(scroll);
        }
        self.overlay = Some(OverlayState::multiline_input(
            MultilineIntent::EditNote {
                task_id,
                display_ref,
                note_id: note_id.to_string(),
            },
            EDIT_NOTE_TITLE,
            "note body:",
            body,
        ));
    }

    pub(super) fn begin_delete_note(&mut self, note_id: &str, scroll: u16) {
        let Some(task_id) = self
            .store
            .selected_task(self.list.selected_task())
            .and_then(|item| {
                item.notes
                    .iter()
                    .any(|note| note.id == note_id)
                    .then(|| item.task.id.clone())
            })
        else {
            self.set_warning("note is unavailable");
            return;
        };
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(scroll);
        }
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::DeleteNote {
                task_id,
                note_id: note_id.to_string(),
            },
            DELETE_NOTE_TITLE,
            "Delete this note?",
        ));
    }

    pub(super) async fn submit_edit_note(
        &mut self,
        task_id: crate::ids::TaskId,
        display_ref: String,
        note_id: String,
        body: String,
    ) -> Result<()> {
        if body.trim().is_empty() {
            self.set_warning("note body is required");
            return Ok(());
        }
        match self.store.edit_note(&task_id, &note_id, body).await? {
            Some(true) => {
                self.refresh().await?;
                self.set_mutation_success(format!("edited note {note_id} on {display_ref}"));
            }
            Some(false) => self.set_info("note is unchanged"),
            None => self.set_warning("note is unavailable"),
        }
        Ok(())
    }

    pub(super) async fn submit_delete_note(
        &mut self,
        task_id: crate::ids::TaskId,
        note_id: String,
    ) -> Result<()> {
        let replacement = self.note_focus_after_delete(&task_id, &note_id);
        if !self.store.delete_note(&task_id, &note_id).await? {
            self.set_warning("note is unavailable");
            return Ok(());
        }
        if let Some(detail) = self.detail.state_mut() {
            detail.set_focused_target(replacement.map(|note_id| DetailTargetId::Note { note_id }));
        }
        self.refresh().await?;
        self.set_mutation_success("deleted note");
        Ok(())
    }

    fn note_focus_after_delete(
        &self,
        task_id: &crate::ids::TaskId,
        note_id: &str,
    ) -> Option<String> {
        let note_ids = self
            .store
            .selected_task(self.list.selected_task())
            .filter(|item| &item.task.id == task_id)?
            .notes
            .iter()
            .map(|note| note.id.clone())
            .collect::<Vec<_>>();
        let index = note_ids.iter().position(|candidate| candidate == note_id)?;
        note_ids.get(index + 1).cloned().or_else(|| {
            index
                .checked_sub(1)
                .and_then(|previous| note_ids.get(previous).cloned())
        })
    }
}

use anyhow::Result;

use crate::operations::{resolve_conflict, task_conflicts};
use crate::tui::store::{ConflictTarget, MutationMessage};
use crate::undo::{UndoCommand, UndoPayload, task_field_value};

use super::TuiStore;

impl TuiStore {
    pub(crate) async fn conflict_targets(
        &self,
        index: Option<usize>,
    ) -> Result<Option<Vec<ConflictTarget>>> {
        let Some(item) = self.selected_task(index) else {
            return Ok(None);
        };
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let details = task_conflicts(&mut conn, &item.task.id, None).await?;
        Ok(Some(
            details
                .into_iter()
                .map(|detail| ConflictTarget {
                    task_id: item.task.id.clone(),
                    display_ref: item.display_ref.clone(),
                    field: detail.field,
                    variant_a: detail.variant_a,
                    local_value: detail.local_value,
                    variant_b: detail.variant_b,
                    remote_value: detail.remote_value,
                })
                .collect(),
        ))
    }

    pub(crate) async fn resolve_conflict_value(
        &mut self,
        target: ConflictTarget,
        value: String,
    ) -> Result<MutationMessage> {
        self.activate_workspace();
        let workspace_id = crate::workspaces::active_workspace_id();
        let mut conn = self.pool.acquire().await?;
        let before =
            task_field_value(&mut conn, &workspace_id, &target.task_id, &target.field).await?;
        let conflict_id =
            crate::undo::conflict_row_id(&mut conn, &workspace_id, &target.task_id, &target.field)
                .await?;
        let outcome = resolve_conflict(&mut conn, &target.task_id, &target.field, &value).await?;
        drop(conn);
        self.record_undo(
            &format!("conflict {} {}", target.display_ref, target.field),
            UndoPayload {
                commands: vec![UndoCommand::RestoreConflictResolution {
                    task_id: target.task_id.clone(),
                    field: target.field.clone(),
                    before,
                    after: value,
                    conflict_id,
                }],
            },
        )
        .await?;
        let selected = self.refresh(Some(&outcome.task.id)).await?;
        Ok(MutationMessage {
            message: format!(
                "resolved {} conflict field={}",
                target.display_ref, outcome.field
            ),
            selected,
        })
    }

    pub(crate) fn next_conflict_flag_index(
        flags: &[bool],
        selected: Option<usize>,
        delta: isize,
    ) -> Option<usize> {
        if flags.is_empty() || !flags.iter().any(|flag| *flag) {
            return None;
        }
        let len = flags.len() as isize;
        let mut current = selected.unwrap_or(0).min(flags.len() - 1) as isize;
        for _ in 0..len {
            current = (current + delta).rem_euclid(len);
            if flags[current as usize] {
                return Some(current as usize);
            }
        }
        None
    }

    pub(crate) fn next_conflict_index(
        &self,
        selected: Option<usize>,
        delta: isize,
    ) -> Option<usize> {
        let flags = self
            .tasks
            .iter()
            .map(|task| task.has_conflict)
            .collect::<Vec<_>>();
        Self::next_conflict_flag_index(&flags, selected, delta)
    }
}

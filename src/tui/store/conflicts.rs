use anyhow::Result;

use crate::tui::store::{ConflictTarget, MutationMessage};
use crate::undo::UndoCommand;

use super::TuiStore;

impl TuiStore {
    pub(crate) async fn conflict_targets(
        &self,
        index: Option<usize>,
    ) -> Result<Option<Vec<ConflictTarget>>> {
        let Some(item) = self.selected_task(index) else {
            return Ok(None);
        };
        let details = self
            .database
            .task_conflicts(&self.active_workspace, &item.task.id, None)
            .await?;
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
        let resolution = self
            .database
            .resolve_conflict_for_undo(
                &self.active_workspace,
                &target.task_id,
                &target.field,
                &value,
            )
            .await?;
        let resolved_task_id = resolution.outcome.task.id.clone();
        let resolved_field = resolution.outcome.field.clone();
        self.record_undo_commands(
            &format!("conflict {} {}", target.display_ref, target.field),
            vec![UndoCommand::RestoreConflictResolution {
                task_id: target.task_id.clone(),
                field: target.field.clone(),
                before: resolution.before,
                after: resolution.after,
                conflict_id: resolution.conflict_id,
            }],
        )
        .await?;
        self.refresh_task_message(
            &resolved_task_id,
            format!(
                "resolved {} conflict field={}",
                target.display_ref, resolved_field
            ),
        )
        .await
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

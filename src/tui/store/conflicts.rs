use anyhow::Result;

use crate::tui::store::{ConflictTarget, MutationMessage};

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
        let mut targets = details
            .into_iter()
            .map(|detail| ConflictTarget {
                task_id: item.task.id.clone(),
                recurrence_series_id: None,
                display_ref: item.display_ref.clone(),
                field: detail.field,
                variant_a: detail.variant_a,
                local_value: detail.local_value,
                variant_b: detail.variant_b,
                remote_value: detail.remote_value,
            })
            .collect::<Vec<_>>();
        if let Some(recurrence) = item.recurrence.as_ref() {
            let series_details = self
                .database
                .recurrence_series_conflicts(&self.active_workspace, &recurrence.series_id, None)
                .await?;
            targets.extend(series_details.into_iter().map(|detail| ConflictTarget {
                task_id: item.task.id.clone(),
                recurrence_series_id: Some(recurrence.series_id.clone()),
                display_ref: recurrence.series_ref.clone(),
                field: detail.field,
                variant_a: detail.variant_a,
                local_value: detail.local_value,
                variant_b: detail.variant_b,
                remote_value: detail.remote_value,
            }));
        }
        Ok(Some(targets))
    }

    pub(crate) async fn append_recurrence_conflict_tasks(&mut self) -> Result<()> {
        let project = self.scope_project().map(str::to_string);
        let conflicts = self
            .database
            .list_conflicts(&self.active_workspace, project.as_deref(), None)
            .await?;
        let mut task_ids = Vec::new();
        for conflict in conflicts
            .into_iter()
            .filter(|conflict| conflict.recurrence_series)
        {
            let series_id = conflict.task_id.to_string().parse()?;
            let detail = self
                .database
                .recurrence_series_detail(&self.active_workspace.id, &series_id)
                .await?;
            if let Some(task_id) = detail
                .current_occurrence
                .and_then(|occurrence| occurrence.task_id)
                && !task_ids.contains(&task_id)
            {
                task_ids.push(task_id);
            }
        }
        if task_ids.is_empty() {
            return Ok(());
        }
        let mut items = self
            .database
            .list_task_items(
                &self.active_workspace.id,
                crate::query::TaskFilters {
                    task_ids,
                    include_deleted: true,
                    ..crate::query::TaskFilters::default()
                },
                crate::query::TaskQueryMode::Flat,
                crate::query::TaskSort::Created,
                crate::query::SortDirection::Asc,
            )
            .await?;
        for item in &mut items {
            item.has_conflict = true;
        }
        for item in items {
            if let Some(existing) = self
                .tasks
                .iter_mut()
                .find(|existing| existing.task.id == item.task.id)
            {
                existing.has_conflict = true;
            } else {
                self.tasks.push(item);
            }
        }
        Ok(())
    }

    pub(crate) async fn resolve_conflict_value(
        &mut self,
        target: ConflictTarget,
        value: String,
    ) -> Result<MutationMessage> {
        if let Some(series_id) = target.recurrence_series_id.as_ref() {
            let field = target.field.clone();
            self.database
                .resolve_recurrence_conflict(&self.active_workspace, series_id, &field, &value)
                .await?;
            return self
                .refresh_task_message(
                    &target.task_id,
                    format!("resolved {} conflict field={field}", target.display_ref),
                )
                .await;
        }
        let resolution = self
            .database
            .resolve_conflict_with_tui_undo(
                &self.active_workspace,
                &target.task_id,
                &target.field,
                &value,
                &format!("conflict {} {}", target.display_ref, target.field),
            )
            .await?;
        let resolved_task_id = resolution.outcome.task.id.clone();
        let resolved_field = resolution.outcome.field.clone();
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

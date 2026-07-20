use anyhow::{Context, Result};
use aven_core::operations::{RecurrenceSeriesDraft, RecurrenceTemplateUpdate};
use aven_core::query::{RecurrenceHistoryKind, RecurrenceHistoryPage, RecurrenceSeriesDetail};
use aven_core::recurrence::{RecurrenceOutcome, RecurrenceSchedule, RecurrenceSeriesId};
use chrono::{DateTime, NaiveDate, Utc};

use super::{MutationMessage, TuiStore};

impl TuiStore {
    pub(crate) async fn create_recurrence_series(
        &mut self,
        mut draft: RecurrenceSeriesDraft,
        current_selected_index: Option<usize>,
    ) -> Result<(String, Option<usize>)> {
        if draft.project.is_empty() {
            draft.project = self.inferred_add_project().await?.unwrap_or_default();
        }
        let previous_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        let outcome = self
            .database
            .create_recurrence_series(&self.active_workspace, draft)
            .await?;
        let task_id = outcome.task.id.clone();
        self.refresh(None).await?;
        let selected = self
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .or_else(|| self.restored_task_selection(previous_id.as_ref()));
        let suffix = if self.tasks.iter().any(|item| item.task.id == task_id) {
            ""
        } else {
            " hidden by current filters"
        };
        Ok((
            format!(
                "created recurring task {} ({}){suffix}",
                outcome.series_ref, outcome.task.id
            ),
            selected,
        ))
    }

    pub(crate) async fn recurrence_detail_for_task(
        &self,
        index: Option<usize>,
    ) -> Result<Option<RecurrenceSeriesDetail>> {
        let Some(summary) = self
            .selected_task(index)
            .and_then(|item| item.recurrence.as_ref())
        else {
            return Ok(None);
        };
        self.database
            .recurrence_series_detail(&self.active_workspace.id, &summary.series_id)
            .await
            .map(Some)
    }

    pub(crate) async fn recurrence_history_for_task(
        &self,
        index: Option<usize>,
    ) -> Result<Option<RecurrenceHistoryPage>> {
        let Some(summary) = self
            .selected_task(index)
            .and_then(|item| item.recurrence.as_ref())
        else {
            return Ok(None);
        };
        self.database
            .recurrence_history(&self.active_workspace.id, &summary.series_id, 0, 200)
            .await
            .map(Some)
    }

    pub(crate) async fn update_recurrence_template(
        &mut self,
        series_id: &RecurrenceSeriesId,
        update: RecurrenceTemplateUpdate,
        selected_task_id: Option<&crate::ids::TaskId>,
    ) -> Result<MutationMessage> {
        let series_ref = self
            .database
            .recurrence_series_ref(&self.active_workspace.id, series_id)
            .await?;
        let outcome = self
            .database
            .update_recurrence_template(&self.active_workspace, series_id, update)
            .await?;
        let selected = self.refresh(selected_task_id).await?;
        let verb = if outcome.changed { "updated" } else { "kept" };
        Ok(MutationMessage::new(
            format!("{verb} recurring template {series_ref}"),
            selected,
        ))
    }

    pub(crate) async fn skip_recurrence(
        &mut self,
        index: Option<usize>,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let Some(summary) = item.recurrence.as_ref() else {
            return Ok(None);
        };
        self.database
            .resolve_recurrence_occurrence_with_undo(
                &self.active_workspace,
                &item.task.id,
                RecurrenceOutcome::Skipped,
                crate::undo::UndoContext::tui(format!("skip {}", summary.series_ref)),
            )
            .await?;
        let selected = self.refresh(None).await?;
        Ok(Some(MutationMessage::new(
            format!("skipped {} slot {}", summary.series_ref, summary.slot_on),
            selected,
        )))
    }

    pub(crate) async fn record_recurrence_outcome(
        &mut self,
        series_id: &RecurrenceSeriesId,
        slot_on: NaiveDate,
        outcome: RecurrenceOutcome,
        resolved_at: Option<String>,
    ) -> Result<MutationMessage> {
        let now = Utc::now();
        let resolved_at = resolved_at
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|value| value.with_timezone(&Utc).to_rfc3339())
                    .context("use an RFC 3339 outcome time")
            })
            .transpose()?
            .unwrap_or_else(|| now.to_rfc3339());
        let result = self
            .database
            .record_recurrence_outcome(
                &self.active_workspace,
                series_id,
                slot_on,
                outcome,
                resolved_at,
                now,
            )
            .await?;
        let series_ref = self
            .database
            .recurrence_series_ref(&self.active_workspace.id, &result.series.id)
            .await?;
        let selected = self.refresh(None).await?;
        Ok(MutationMessage::new(
            format!("recorded {} {} on {slot_on}", series_ref, outcome.as_str()),
            selected,
        ))
    }

    pub(crate) async fn pause_recurrence(
        &mut self,
        index: Option<usize>,
    ) -> Result<Option<MutationMessage>> {
        self.set_recurrence_state(index, RecurrenceStateAction::Pause)
            .await
    }

    pub(crate) async fn resume_recurrence(
        &mut self,
        index: Option<usize>,
    ) -> Result<Option<MutationMessage>> {
        self.set_recurrence_state(index, RecurrenceStateAction::Resume)
            .await
    }

    pub(crate) async fn stop_recurrence(
        &mut self,
        index: Option<usize>,
    ) -> Result<Option<MutationMessage>> {
        self.set_recurrence_state(index, RecurrenceStateAction::Stop)
            .await
    }

    async fn set_recurrence_state(
        &mut self,
        index: Option<usize>,
        action: RecurrenceStateAction,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let Some(summary) = item.recurrence.as_ref() else {
            return Ok(None);
        };
        match action {
            RecurrenceStateAction::Pause => {
                self.database
                    .pause_recurrence_series(&self.active_workspace, &summary.series_id)
                    .await?;
            }
            RecurrenceStateAction::Resume => {
                self.database
                    .resume_recurrence_series(
                        &self.active_workspace,
                        &summary.series_id,
                        Utc::now(),
                    )
                    .await?;
            }
            RecurrenceStateAction::Stop => {
                self.database
                    .stop_recurrence_series(&self.active_workspace, &summary.series_id, false)
                    .await?;
            }
        }
        let selected = self.refresh(Some(&item.task.id)).await?;
        Ok(Some(MutationMessage::new(
            format!("{} recurring series {}", action.verb(), summary.series_ref),
            selected,
        )))
    }
}

#[derive(Clone, Copy)]
enum RecurrenceStateAction {
    Pause,
    Resume,
    Stop,
}

impl RecurrenceStateAction {
    fn verb(self) -> &'static str {
        match self {
            Self::Pause => "paused",
            Self::Resume => "resumed",
            Self::Stop => "stopped",
        }
    }
}

pub(crate) fn recurrence_history_lines(page: &RecurrenceHistoryPage) -> Vec<String> {
    let mut lines = vec![format!("series {}", page.series_ref)];
    for item in &page.items {
        let kind = match item.kind {
            RecurrenceHistoryKind::Completed => "completed",
            RecurrenceHistoryKind::Skipped => "skipped",
            RecurrenceHistoryKind::Missed => "missed",
            RecurrenceHistoryKind::Paused => "paused",
        };
        if let Some(slot) = item.slot_on.as_deref() {
            let task_ref = item
                .task_ref
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default();
            let corrected = if item.corrected { " corrected" } else { "" };
            let archived = if item.archived_projection {
                " archived projection"
            } else {
                ""
            };
            lines.push(format!("{slot}  {kind}{task_ref}{corrected}{archived}"));
        } else {
            lines.push(format!(
                "{}  paused until {}",
                item.interval_started_at.as_deref().unwrap_or("unknown"),
                item.interval_ended_at.as_deref().unwrap_or("present")
            ));
        }
    }
    if page.has_more {
        lines.push(format!("{} more entries", page.total - page.items.len()));
    }
    lines
}

pub(crate) fn recurrence_draft(
    title: String,
    description: String,
    project: Option<String>,
    priority: String,
    status: String,
    labels: Vec<String>,
    schedule: RecurrenceSchedule,
) -> RecurrenceSeriesDraft {
    RecurrenceSeriesDraft {
        title,
        description,
        project: project.unwrap_or_default(),
        priority,
        initial_status: status,
        labels,
        schedule,
    }
}

use anyhow::{Context, Result};
use aven_core::operations::{RecurrenceSeriesDraft, RecurrenceTemplateUpdate};
use aven_core::query::{RecurrenceHistoryPage, RecurrenceSeriesDetail};
use aven_core::recurrence::{RecurrenceOutcome, RecurrenceSchedule, RecurrenceSeriesId};
use chrono::{DateTime, NaiveDate, Utc};

use super::{MutationMessage, TuiStore};

impl TuiStore {
    pub(crate) async fn load_recurrence_series_detail(
        &mut self,
        series_id: &RecurrenceSeriesId,
    ) -> Result<()> {
        let detail = self
            .database
            .recurrence_series_detail(&self.active_workspace.id, series_id)
            .await?;
        anyhow::ensure!(
            &detail.series.id == series_id,
            "recurrence detail identity changed while loading"
        );
        self.recurrence_detail = Some(detail);
        Ok(())
    }

    pub(crate) async fn create_recurrence_series(
        &mut self,
        mut draft: RecurrenceSeriesDraft,
        current_selected_index: Option<usize>,
    ) -> Result<(String, Option<usize>)> {
        if draft.project.is_empty() {
            draft.project = self
                .inferred_add_project()
                .await?
                .context("choose a project for this recurring task")?;
        }
        let recurring_view = self.view_state.view == super::TaskView::Recurring;
        let previous_task_id = (!recurring_view)
            .then(|| {
                self.selected_task(current_selected_index)
                    .map(|item| item.task.id.clone())
            })
            .flatten();
        let previous_series_id = recurring_view
            .then(|| {
                self.selected_recurrence_series(current_selected_index)
                    .map(|item| item.series.id.clone())
            })
            .flatten();
        let outcome = self
            .database
            .create_recurrence_series(&self.active_workspace, draft)
            .await?;
        self.wake_after_mutation();
        if recurring_view {
            let created_id = outcome.series.id.clone();
            let requested = super::MainRowSelection::RecurrenceSeries(created_id.clone());
            let refreshed = self.refresh_with_scope_fallback(Some(&requested)).await?;
            let created_index = self
                .recurrence_series
                .iter()
                .position(|item| item.series.id == created_id);
            let previous_index = previous_series_id.as_ref().and_then(|series_id| {
                self.recurrence_series
                    .iter()
                    .position(|item| &item.series.id == series_id)
            });
            let selected = created_index.or(previous_index).or(refreshed.selected);
            return Ok((
                format!("Created recurring task {}", outcome.series_ref),
                selected,
            ));
        }
        let task_id = outcome.task.id.clone();
        self.refresh(None).await?;
        let selected = self
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            .or_else(|| self.restored_task_selection(previous_task_id.as_ref()));
        Ok((
            format!("Created recurring task {}", outcome.series_ref),
            selected,
        ))
    }

    pub(crate) async fn recurrence_detail_for_series(
        &self,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceSeriesDetail> {
        self.database
            .recurrence_series_detail(&self.active_workspace.id, series_id)
            .await
    }

    pub(crate) async fn find_recurrence_project(
        &self,
        detail: &RecurrenceSeriesDetail,
    ) -> Result<Option<aven_core::types::Project>> {
        self.database
            .find_project_by_id(&self.active_workspace.id, &detail.series.project_id)
            .await
    }

    pub(crate) async fn recurrence_history_for_series(
        &self,
        series_id: &RecurrenceSeriesId,
        as_of: DateTime<Utc>,
        offset: usize,
        limit: usize,
    ) -> Result<RecurrenceHistoryPage> {
        self.database
            .recurrence_history_at(&self.active_workspace.id, series_id, as_of, offset, limit)
            .await
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
        self.wake_after_mutation();
        let selected = self
            .refresh_after_recurrence_mutation(series_id, selected_task_id)
            .await?;
        if self
            .recurrence_detail
            .as_ref()
            .is_some_and(|detail| detail.series.id == *series_id)
        {
            self.load_recurrence_series_detail(series_id).await?;
        }
        let verb = if outcome.changed { "updated" } else { "kept" };
        Ok(MutationMessage::new(
            format!("{verb} recurring template {series_ref}"),
            selected,
        ))
    }

    pub(crate) async fn skip_recurrence(
        &mut self,
        series_id: &RecurrenceSeriesId,
        task_id: &crate::ids::TaskId,
        slot_on: NaiveDate,
    ) -> Result<MutationMessage> {
        let series_ref = self
            .database
            .recurrence_series_ref(&self.active_workspace.id, series_id)
            .await?;
        self.database
            .resolve_recurrence_occurrence_with_undo(
                &self.active_workspace,
                task_id,
                RecurrenceOutcome::Skipped,
                crate::undo::UndoContext::tui(format!("skip {series_ref}")),
            )
            .await?;
        self.wake_after_mutation();
        let selected = self
            .refresh_after_recurrence_mutation(series_id, Some(task_id))
            .await?;
        Ok(MutationMessage::new(
            format!("skipped {series_ref} slot {slot_on}"),
            selected,
        ))
    }

    pub(crate) async fn pause_recurrence(
        &mut self,
        series_id: &RecurrenceSeriesId,
        selected_task_id: Option<&crate::ids::TaskId>,
    ) -> Result<MutationMessage> {
        self.set_recurrence_state(series_id, selected_task_id, RecurrenceStateAction::Pause)
            .await
    }

    pub(crate) async fn resume_recurrence(
        &mut self,
        series_id: &RecurrenceSeriesId,
        selected_task_id: Option<&crate::ids::TaskId>,
    ) -> Result<MutationMessage> {
        self.set_recurrence_state(series_id, selected_task_id, RecurrenceStateAction::Resume)
            .await
    }

    pub(crate) async fn stop_recurrence(
        &mut self,
        series_id: &RecurrenceSeriesId,
        selected_task_id: Option<&crate::ids::TaskId>,
        skip_current: bool,
    ) -> Result<MutationMessage> {
        let series_ref = self
            .database
            .recurrence_series_ref(&self.active_workspace.id, series_id)
            .await?;
        self.database
            .stop_recurrence_series(&self.active_workspace, series_id, skip_current)
            .await?;
        self.wake_after_mutation();
        if self.view_state.view == super::TaskView::Recurring {
            self.view_state.recurring.lifecycle =
                aven_core::query::RecurrenceSeriesLifecycleFilter::All;
        }
        let selected = self
            .refresh_after_recurrence_mutation(series_id, selected_task_id)
            .await?;
        let outcome = if skip_current {
            "skipped current occurrence"
        } else {
            "kept current occurrence"
        };
        Ok(MutationMessage::new(
            format!("stopped recurring series {series_ref}; {outcome}"),
            selected,
        ))
    }

    async fn set_recurrence_state(
        &mut self,
        series_id: &RecurrenceSeriesId,
        selected_task_id: Option<&crate::ids::TaskId>,
        action: RecurrenceStateAction,
    ) -> Result<MutationMessage> {
        let series_ref = self
            .database
            .recurrence_series_ref(&self.active_workspace.id, series_id)
            .await?;
        match action {
            RecurrenceStateAction::Pause => {
                self.database
                    .pause_recurrence_series(&self.active_workspace, series_id)
                    .await?;
            }
            RecurrenceStateAction::Resume => {
                self.database
                    .resume_recurrence_series(&self.active_workspace, series_id, Utc::now())
                    .await?;
            }
        }
        self.wake_after_mutation();
        let selected = self
            .refresh_after_recurrence_mutation(series_id, selected_task_id)
            .await?;
        Ok(MutationMessage::new(
            format!("{} recurring series {series_ref}", action.verb()),
            selected,
        ))
    }

    async fn refresh_after_recurrence_mutation(
        &mut self,
        series_id: &RecurrenceSeriesId,
        selected_task_id: Option<&crate::ids::TaskId>,
    ) -> Result<Option<usize>> {
        if self.view_state.view == super::TaskView::Recurring {
            return Ok(self
                .refresh_with_scope_fallback(Some(&super::MainRowSelection::RecurrenceSeries(
                    series_id.clone(),
                )))
                .await?
                .selected);
        }
        self.refresh(selected_task_id).await
    }
}

#[derive(Clone, Copy)]
enum RecurrenceStateAction {
    Pause,
    Resume,
}

impl RecurrenceStateAction {
    fn verb(self) -> &'static str {
        match self {
            Self::Pause => "paused",
            Self::Resume => "resumed",
        }
    }
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

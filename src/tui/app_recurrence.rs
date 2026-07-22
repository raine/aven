use anyhow::{Context, Result, bail};
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::recurrence::{
    RecurrenceOutcome, RecurrenceProjectionState, RecurrenceSeriesId, RecurrenceSeriesState,
};
use chrono::{DateTime, NaiveDate};

use crate::tui::app::{App, SeriesDetailReturn};
use crate::tui::event::Action;
use crate::tui::overlay::{
    CommandAvailabilityOverride, OverlayState, OverlayTarget, PickerIntent, PickerItem,
    TextIntent, TextPanelState,
};
use crate::tui::store::TaskView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecurrenceActionKind {
    SkipCurrent,
    RecordHistorical,
    EditTemplate,
    Pause,
    Resume,
    Stop,
    History,
}

impl RecurrenceActionKind {
    const ALL: [Self; 7] = [
        Self::SkipCurrent,
        Self::RecordHistorical,
        Self::EditTemplate,
        Self::Pause,
        Self::Resume,
        Self::Stop,
        Self::History,
    ];

    fn value(self) -> &'static str {
        match self {
            Self::SkipCurrent => "skip",
            Self::RecordHistorical => "record",
            Self::EditTemplate => "edit-template",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
            Self::History => "history",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::SkipCurrent => "Skip current occurrence",
            Self::RecordHistorical => "Record historical outcome",
            Self::EditTemplate => "Edit template",
            Self::Pause => "Pause series",
            Self::Resume => "Resume series",
            Self::Stop => "Stop series permanently",
            Self::History => "Show history",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.value() == value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecurrenceTargetId {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) series_id: RecurrenceSeriesId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CurrentOccurrenceTarget {
    task_id: TaskId,
    slot_on: NaiveDate,
}

#[derive(Clone, Debug)]
struct RecurrenceTarget {
    id: RecurrenceTargetId,
    series_ref: String,
    state: RecurrenceSeriesState,
    current: Option<CurrentOccurrenceTarget>,
    project_key: String,
    detail: aven_core::query::RecurrenceSeriesDetail,
}

impl RecurrenceTarget {
    fn unavailable_reason(&self, action: RecurrenceActionKind) -> Option<&'static str> {
        recurrence_unavailable_reason(self.state, self.current.is_some(), action)
    }

    fn selected_task_id(&self) -> Option<&TaskId> {
        self.current.as_ref().map(|current| &current.task_id)
    }
}

fn recurrence_unavailable_reason(
    state: RecurrenceSeriesState,
    has_current: bool,
    action: RecurrenceActionKind,
) -> Option<&'static str> {
    use RecurrenceActionKind::{
        EditTemplate, History, Pause, RecordHistorical, Resume, SkipCurrent, Stop,
    };
    use RecurrenceSeriesState::{Active, Paused, Stopped};

    match (state, action) {
        (_, History | RecordHistorical | EditTemplate) => None,
        (_, SkipCurrent) if has_current => None,
        (_, SkipCurrent) => Some("series has no current occurrence to skip"),
        (Active, Pause | Stop) => None,
        (Active, Resume) => Some("series is already active"),
        (Paused, Resume | Stop) => None,
        (Paused, Pause) => Some("series is already paused"),
        (Stopped, Stop) => Some("series is already stopped"),
        (Stopped, Pause | Resume) => Some("series is stopped"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopDisposition {
    KeepCurrent,
    SkipCurrent,
}

impl TryFrom<&str> for StopDisposition {
    type Error = &'static str;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "keep" => Ok(Self::KeepCurrent),
            "skip" => Ok(Self::SkipCurrent),
            _ => Err("invalid stop outcome"),
        }
    }
}

fn parse_historical_outcome(value: &str) -> Result<(NaiveDate, RecurrenceOutcome, Option<String>)> {
    let words = value.split_whitespace().collect::<Vec<_>>();
    if !(2..=3).contains(&words.len()) {
        bail!("use YYYY-MM-DD completed|skipped [RFC3339 time]");
    }
    let slot = NaiveDate::parse_from_str(words[0], "%Y-%m-%d")
        .context("use a real historical date in YYYY-MM-DD form")?;
    let outcome = match words[1] {
        "completed" => RecurrenceOutcome::Completed,
        "skipped" => RecurrenceOutcome::Skipped,
        _ => bail!("outcome must be completed or skipped"),
    };
    let resolved_at = words
        .get(2)
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .context("historical time must use RFC3339 form")
                .map(|parsed| parsed.to_rfc3339())
        })
        .transpose()?;
    Ok((slot, outcome, resolved_at))
}

impl App {
    pub(super) async fn open_recurrence_occurrence(&mut self) -> Result<()> {
        let Some(detail) = self.store.recurrence_detail.as_ref() else {
            self.set_warning("recurring series detail is unavailable");
            return Ok(());
        };
        let Some(task_id) = detail
            .current_occurrence
            .as_ref()
            .and_then(|occurrence| occurrence.task_id.clone())
        else {
            self.set_warning("recurring series has no applicable occurrence task");
            return Ok(());
        };
        let series_id = detail.series.id.clone();
        let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
        let previous = self.store.view_state.clone();
        let selected = self.store.show_task_by_id(task_id).await?;
        let Some(selected) = selected else {
            let restore = crate::tui::store::MainRowSelection::RecurrenceSeries(series_id.clone());
            self.store
                .restore_view_state(previous, Some(&restore))
                .await?;
            self.store.load_recurrence_series_detail(&series_id).await?;
            self.set_warning("occurrence task is unavailable");
            return Ok(());
        };
        self.push_navigation_state(previous);
        self.series_detail_return = Some(SeriesDetailReturn { series_id, scroll });
        self.list.select_task(Some(selected));
        self.detail = crate::tui::detail_session::DetailSession::open(0);
        self.overlay = None;
        Ok(())
    }

    pub(crate) fn selected_recurrence_target_id(&self) -> Option<RecurrenceTargetId> {
        let workspace_id = self.store.active_workspace.id.clone();
        let series_id = if self.store.view_state.view == TaskView::Recurring {
            self.store
                .selected_recurrence_series(self.list.selected_task())
                .map(|item| item.series.id.clone())
        } else {
            self.store
                .selected_task(self.list.selected_task())
                .and_then(|item| item.recurrence.as_ref())
                .map(|summary| summary.series_id.clone())
        }?;
        Some(RecurrenceTargetId {
            workspace_id,
            series_id,
        })
    }

    pub(crate) fn recurrence_command_context(
        &self,
    ) -> (Option<OverlayTarget>, Vec<CommandAvailabilityOverride>) {
        let selected = if self.store.view_state.view == TaskView::Recurring {
            self.store
                .selected_recurrence_series(self.list.selected_task())
                .map(|item| {
                    (
                        item.series.id.clone(),
                        item.series.state,
                        item.current_occurrence.is_some(),
                    )
                })
        } else {
            self.store
                .selected_task(self.list.selected_task())
                .and_then(|item| item.recurrence.as_ref())
                .map(|summary| {
                    (
                        summary.series_id.clone(),
                        summary.lifecycle,
                        summary.outcome.is_none()
                            && matches!(
                                summary.projection_state,
                                RecurrenceProjectionState::Projected
                            ),
                    )
                })
        };
        let Some((series_id, state, has_current)) = selected else {
            return (None, Vec::new());
        };
        let target = RecurrenceTargetId {
            workspace_id: self.store.active_workspace.id.clone(),
            series_id,
        };
        let actions = [
            Action::SkipRecurrence,
            Action::BeginRecordRecurrence,
            Action::BeginEditRecurrenceTemplate,
            Action::PauseRecurrence,
            Action::ResumeRecurrence,
            Action::StopRecurrence,
            Action::ShowRecurrenceHistory,
        ];
        let unavailable = actions
            .into_iter()
            .filter_map(|action| {
                let kind = action
                    .recurrence_kind()
                    .expect("recurrence command has a recurrence action kind");
                recurrence_unavailable_reason(state, has_current, kind)
                    .map(|reason| CommandAvailabilityOverride { action, reason })
            })
            .collect();
        (Some(Self::overlay_target(&target)), unavailable)
    }

    fn overlay_target(target: &RecurrenceTargetId) -> OverlayTarget {
        OverlayTarget::RecurrenceSeries {
            workspace_id: target.workspace_id.clone(),
            series_id: target.series_id.clone(),
        }
    }

    fn recurrence_target_id(target: Option<OverlayTarget>) -> Option<RecurrenceTargetId> {
        target.map(
            |OverlayTarget::RecurrenceSeries {
                 workspace_id,
                 series_id,
             }| RecurrenceTargetId {
                workspace_id,
                series_id,
            },
        )
    }

    async fn load_recurrence_target(
        &self,
        id: &RecurrenceTargetId,
    ) -> Result<Option<RecurrenceTarget>> {
        if id.workspace_id != self.store.active_workspace.id {
            return Ok(None);
        }
        let detail = self
            .store
            .recurrence_detail_for_series(&id.series_id)
            .await?;
        let project_key = self.store.recurrence_project_key(&detail).await?;
        let current = detail.current_occurrence.as_ref().and_then(|occurrence| {
            if occurrence.outcome.is_some()
                || !matches!(
                    occurrence.projection_state,
                    RecurrenceProjectionState::Projected
                )
            {
                return None;
            }
            Some(CurrentOccurrenceTarget {
                task_id: occurrence.task_id.clone()?,
                slot_on: occurrence.slot_on,
            })
        });
        Ok(Some(RecurrenceTarget {
            id: id.clone(),
            series_ref: detail.summary.series_ref.clone(),
            state: detail.series.state,
            current,
            project_key,
            detail,
        }))
    }

    pub(super) async fn run_recurrence_action(
        &mut self,
        id: RecurrenceTargetId,
        action: RecurrenceActionKind,
    ) -> Result<()> {
        let Some(target) = self.load_recurrence_target(&id).await? else {
            self.set_warning("recurring series target is unavailable");
            return Ok(());
        };
        if let Some(reason) = target.unavailable_reason(action) {
            self.set_warning(reason);
            return Ok(());
        }

        match action {
            RecurrenceActionKind::SkipCurrent => {
                let current = target
                    .current
                    .as_ref()
                    .expect("availability requires a current occurrence");
                let message = self
                    .store
                    .skip_recurrence(&id.series_id, &current.task_id, current.slot_on)
                    .await?;
                self.apply_recurrence_message(&id.series_id, message)
                    .await?;
            }
            RecurrenceActionKind::RecordHistorical => self.begin_record_recurrence(&target),
            RecurrenceActionKind::EditTemplate => {
                self.authoring
                    .begin_edit_recurrence_template(&target.detail, target.project_key);
                self.begin_add_task_step();
            }
            RecurrenceActionKind::Pause => {
                let message = self
                    .store
                    .pause_recurrence(&id.series_id, target.selected_task_id())
                    .await?;
                self.apply_recurrence_message(&id.series_id, message)
                    .await?;
            }
            RecurrenceActionKind::Resume => {
                let message = self
                    .store
                    .resume_recurrence(&id.series_id, target.selected_task_id())
                    .await?;
                self.apply_recurrence_message(&id.series_id, message)
                    .await?;
            }
            RecurrenceActionKind::Stop => self.begin_stop_recurrence(&target),
            RecurrenceActionKind::History => {
                let page = self
                    .store
                    .recurrence_history_for_series(&id.series_id)
                    .await?;
                self.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
                    format!("Recurrence history: {}", page.series_ref),
                    crate::tui::store::recurrence_history_lines(&page),
                )));
            }
        }
        Ok(())
    }

    async fn apply_recurrence_message(
        &mut self,
        series_id: &RecurrenceSeriesId,
        message: crate::tui::store::MutationMessage,
    ) -> Result<()> {
        self.list.select_task(message.selected);
        if self.store.view_state.view == TaskView::Recurring
            && self.store.recurrence_detail.is_some()
        {
            self.store.load_recurrence_series_detail(series_id).await?;
        }
        self.set_success(message.message);
        Ok(())
    }

    pub(super) async fn execute_selected_recurrence_action(
        &mut self,
        action: Action,
    ) -> Result<()> {
        let Some(kind) = action.recurrence_kind() else {
            return Ok(());
        };
        let Some(target) = self.selected_recurrence_target_id() else {
            self.set_warning("no selected recurring series");
            return Ok(());
        };
        self.run_recurrence_action(target, kind).await
    }

    pub(super) async fn execute_targeted_recurrence_action(
        &mut self,
        target: Option<OverlayTarget>,
        action: Action,
    ) -> Result<()> {
        let Some(id) = Self::recurrence_target_id(target) else {
            self.set_warning("recurring series target is unavailable");
            return Ok(());
        };
        let Some(kind) = action.recurrence_kind() else {
            return Ok(());
        };
        self.run_recurrence_action(id, kind).await
    }

    pub(super) async fn begin_recurrence_context_menu(
        &mut self,
        id: RecurrenceTargetId,
    ) -> Result<()> {
        let Some(target) = self.load_recurrence_target(&id).await? else {
            self.set_warning("recurring series target is unavailable");
            return Ok(());
        };
        let items = RecurrenceActionKind::ALL
            .into_iter()
            .filter(|action| target.unavailable_reason(*action).is_none())
            .map(|action| PickerItem {
                label: action.label().to_string(),
                value: action.value().to_string(),
                selected: false,
            })
            .collect();
        self.overlay = Some(OverlayState::picker(
            PickerIntent::RecurrenceActions {
                target: Self::overlay_target(&id),
            },
            format!("Actions: {}", target.series_ref),
            items,
            false,
        ));
        Ok(())
    }

    fn begin_stop_recurrence(&mut self, target: &RecurrenceTarget) {
        let mut items = vec![PickerItem {
            label: "Keep current occurrence".to_string(),
            value: "keep".to_string(),
            selected: true,
        }];
        if target.current.is_some() {
            items.push(PickerItem {
                label: "Skip current occurrence".to_string(),
                value: "skip".to_string(),
                selected: false,
            });
        }
        self.overlay = Some(OverlayState::picker(
            PickerIntent::StopRecurrence {
                target: Self::overlay_target(&target.id),
            },
            format!("Stop {} permanently", target.series_ref),
            items,
            false,
        ));
    }

    fn begin_record_recurrence(&mut self, target: &RecurrenceTarget) {
        self.overlay = Some(OverlayState::blank_text_input(
            TextIntent::RecordRecurrenceOutcome {
                target: Self::overlay_target(&target.id),
            },
            format!("Record historical outcome: {}", target.series_ref),
            "YYYY-MM-DD completed|skipped [RFC3339 time]",
        ));
    }

    pub(super) async fn submit_recurrence_action(
        &mut self,
        target: Option<OverlayTarget>,
        value: Option<&str>,
    ) -> Result<()> {
        let Some(id) = Self::recurrence_target_id(target) else {
            self.set_warning("recurring series target is unavailable");
            return Ok(());
        };
        let Some(action) = value.and_then(RecurrenceActionKind::parse) else {
            self.set_warning("invalid recurring series action");
            return Ok(());
        };
        self.run_recurrence_action(id, action).await
    }

    pub(super) async fn submit_stop_recurrence(
        &mut self,
        target: Option<OverlayTarget>,
        value: Option<&str>,
    ) -> Result<()> {
        let Some(id) = Self::recurrence_target_id(target) else {
            self.set_warning("recurring series target is unavailable");
            return Ok(());
        };
        let disposition = match value.map(StopDisposition::try_from) {
            Some(Ok(disposition)) => disposition,
            _ => {
                self.set_warning("invalid stop outcome");
                return Ok(());
            }
        };
        let Some(target) = self.load_recurrence_target(&id).await? else {
            self.set_warning("recurring series target is unavailable");
            return Ok(());
        };
        if let Some(reason) = target.unavailable_reason(RecurrenceActionKind::Stop) {
            self.set_warning(reason);
            return Ok(());
        }
        let skip_current = matches!(disposition, StopDisposition::SkipCurrent);
        if skip_current && target.current.is_none() {
            self.set_warning("series has no current occurrence to skip");
            return Ok(());
        }
        let message = self
            .store
            .stop_recurrence(&id.series_id, target.selected_task_id(), skip_current)
            .await?;
        self.apply_recurrence_message(&id.series_id, message).await
    }

    pub(super) async fn submit_record_recurrence(
        &mut self,
        target: Option<OverlayTarget>,
        value: String,
    ) -> Result<()> {
        let Some(id) = Self::recurrence_target_id(target.clone()) else {
            self.set_warning("recurring series target is unavailable");
            return Ok(());
        };
        let parsed = parse_historical_outcome(&value);
        let (slot, outcome, resolved_at) = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                self.set_warning(error.to_string());
                let Some(recurrence) = self.load_recurrence_target(&id).await? else {
                    return Ok(());
                };
                self.begin_record_recurrence(&recurrence);
                return Ok(());
            }
        };
        let message = self
            .store
            .record_recurrence_outcome(&id.series_id, slot, outcome, resolved_at)
            .await?;
        self.apply_recurrence_message(&id.series_id, message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recurrence_action_availability_matches_lifecycle() {
        use RecurrenceActionKind::{
            EditTemplate, History, Pause, RecordHistorical, Resume, SkipCurrent, Stop,
        };
        use RecurrenceSeriesState::{Active, Paused, Stopped};

        for state in [Active, Paused, Stopped] {
            assert_eq!(
                recurrence_unavailable_reason(state, true, SkipCurrent),
                None
            );
            assert!(recurrence_unavailable_reason(state, false, SkipCurrent).is_some());
            assert_eq!(recurrence_unavailable_reason(state, true, History), None);
            assert_eq!(
                recurrence_unavailable_reason(state, true, RecordHistorical),
                None
            );
            assert_eq!(
                recurrence_unavailable_reason(state, true, EditTemplate),
                None
            );
        }
        assert_eq!(recurrence_unavailable_reason(Active, true, Pause), None);
        assert_eq!(recurrence_unavailable_reason(Active, true, Stop), None);
        assert!(recurrence_unavailable_reason(Active, true, Resume).is_some());
        assert_eq!(recurrence_unavailable_reason(Paused, true, Resume), None);
        assert_eq!(recurrence_unavailable_reason(Paused, true, Stop), None);
        assert!(recurrence_unavailable_reason(Paused, true, Pause).is_some());
        for action in [Pause, Resume, Stop] {
            assert!(recurrence_unavailable_reason(Stopped, true, action).is_some());
        }
    }

    #[test]
    fn historical_outcome_parser_accepts_supported_actions() {
        let (slot, outcome, resolved_at) =
            parse_historical_outcome("2026-07-18 completed 2026-07-18T09:15:00+03:00").unwrap();

        assert_eq!(slot, NaiveDate::from_ymd_opt(2026, 7, 18).unwrap());
        assert_eq!(outcome, RecurrenceOutcome::Completed);
        assert_eq!(resolved_at.as_deref(), Some("2026-07-18T09:15:00+03:00"));
        assert_eq!(
            parse_historical_outcome("2026-07-19 skipped").unwrap().1,
            RecurrenceOutcome::Skipped
        );
    }

    #[test]
    fn historical_outcome_parser_returns_accessible_errors() {
        assert!(
            parse_historical_outcome("2026-07-18")
                .unwrap_err()
                .to_string()
                .contains("YYYY-MM-DD completed|skipped")
        );
        assert!(
            parse_historical_outcome("not-a-date completed")
                .unwrap_err()
                .to_string()
                .contains("real historical date")
        );
        assert!(
            parse_historical_outcome("2026-07-18 missed")
                .unwrap_err()
                .to_string()
                .contains("completed or skipped")
        );
        assert!(
            parse_historical_outcome("2026-07-18 completed tomorrow")
                .unwrap_err()
                .to_string()
                .contains("RFC3339")
        );
    }
}

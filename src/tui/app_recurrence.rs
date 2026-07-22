use anyhow::{Context, Result};
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::recurrence::{
    RecurrenceOutcome, RecurrenceProjectionState, RecurrenceSeriesId, RecurrenceSeriesState,
};
use chrono::{DateTime, NaiveDate, Utc};

use crate::tui::app::{App, SeriesDetailReturn};
use crate::tui::event::Action;
use crate::tui::overlay::{
    CommandAvailabilityOverride, OverlayState, OverlayTarget, PickerIntent, PickerItem, PickerState,
    RECURRENCE_HISTORY_PAGE_SIZE, RecurrenceHistoryAction, RecurrenceHistoryEntryKey,
    RecurrenceHistoryMode, RecurrenceHistoryState, recurrence_history_correction_block_reason,
    recurrence_history_entry_key,
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

fn recurrence_correction_rejection(error: &anyhow::Error) -> Option<&'static str> {
    let message = format!("{error:#}");
    if message.contains("recurrence-correction-not-past") {
        Some("the selected slot is no longer historical")
    } else if message.contains("recurrence-slot-paused") {
        Some("the selected slot falls inside a pause interval")
    } else if message.contains("recurrence-outcome-exists") {
        Some("an outcome already exists for the selected slot")
    } else if message.contains("recurrence-slot-outside-lifetime") {
        Some("the selected slot is outside the series lifetime")
    } else if message.contains("recurrence-slot-off-lattice") {
        Some("the selected slot is outside the recurrence schedule")
    } else {
        None
    }
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
            RecurrenceActionKind::RecordHistorical => {
                self.open_recurrence_history(&target.id, true).await?;
            }
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
                self.open_recurrence_history(&target.id, false).await?;
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

    async fn open_recurrence_history(
        &mut self,
        target: &RecurrenceTargetId,
        prefer_correction: bool,
    ) -> Result<()> {
        let as_of = Utc::now();
        let page = self
            .store
            .recurrence_history_for_series(
                &target.series_id,
                as_of,
                0,
                RECURRENCE_HISTORY_PAGE_SIZE,
            )
            .await?;
        let mut state = RecurrenceHistoryState::new(
            target.workspace_id.clone(),
            target.series_id.clone(),
            as_of,
            page,
        );
        if prefer_correction {
            state.selected = state
                .page
                .items
                .iter()
                .find(|entry| recurrence_history_correction_block_reason(entry).is_none())
                .map(recurrence_history_entry_key)
                .or(state.selected);
            self.set_success("select a missed slot and press c to record an outcome");
        }
        self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
        Ok(())
    }

    async fn load_recurrence_history_page(
        &mut self,
        mut state: RecurrenceHistoryState,
        offset: usize,
        preferred: Option<RecurrenceHistoryEntryKey>,
        fallback_index: usize,
    ) -> Result<()> {
        let mut page = self
            .store
            .recurrence_history_for_series(
                &state.series_id,
                state.as_of,
                offset,
                RECURRENCE_HISTORY_PAGE_SIZE,
            )
            .await?;
        if page.items.is_empty() && page.offset > 0 && page.total > 0 {
            let last_offset = ((page.total - 1) / page.limit) * page.limit;
            page = self
                .store
                .recurrence_history_for_series(
                    &state.series_id,
                    state.as_of,
                    last_offset,
                    RECURRENCE_HISTORY_PAGE_SIZE,
                )
                .await?;
        }
        state.replace_page(page, preferred, fallback_index);
        self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
        Ok(())
    }

    pub(super) async fn handle_recurrence_history_key(
        &mut self,
        mut state: RecurrenceHistoryState,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use crossterm::event::{KeyCode, KeyModifiers};

        match &mut state.mode {
            RecurrenceHistoryMode::Browse => match key.code {
                KeyCode::Esc => {}
                KeyCode::Down | KeyCode::Char('j') => {
                    state.move_selection(1);
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    state.move_selection(-1);
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
                KeyCode::PageDown if state.page.has_more => {
                    let offset = state.page.offset.saturating_add(state.page.limit);
                    self.load_recurrence_history_page(state, offset, None, 0)
                        .await?;
                }
                KeyCode::PageUp if state.page.offset > 0 => {
                    let offset = state.page.offset.saturating_sub(state.page.limit);
                    self.load_recurrence_history_page(state, offset, None, 0)
                        .await?;
                }
                KeyCode::Enter => {
                    self.run_recurrence_history_action(state, RecurrenceHistoryAction::OpenTask)
                        .await?;
                }
                KeyCode::Char('c') if key.modifiers == KeyModifiers::NONE => {
                    self.run_recurrence_history_action(state, RecurrenceHistoryAction::Correct)
                        .await?;
                }
                _ => {
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
            },
            RecurrenceHistoryMode::Outcome { slot_on, picker } => match key.code {
                KeyCode::Esc => {
                    state.mode = RecurrenceHistoryMode::Browse;
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    picker.selected = picker
                        .selected
                        .saturating_add(1)
                        .min(picker.items.len().saturating_sub(1));
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    picker.selected = picker.selected.saturating_sub(1);
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
                KeyCode::Enter => {
                    let outcome = picker.items.get(picker.selected).and_then(|item| {
                        match item.value.as_str() {
                            "completed" => Some(RecurrenceOutcome::Completed),
                            "skipped" => Some(RecurrenceOutcome::Skipped),
                            _ => None,
                        }
                    });
                    if let Some(outcome) = outcome {
                        state.mode = RecurrenceHistoryMode::ResolutionTime {
                            slot_on: *slot_on,
                            outcome,
                            input: crate::tui::overlay::LineEdit::blank(),
                        };
                    }
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
                _ => {
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
            },
            RecurrenceHistoryMode::ResolutionTime {
                slot_on,
                outcome,
                input,
            } => match key.code {
                KeyCode::Esc => {
                    state.mode = RecurrenceHistoryMode::Browse;
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
                KeyCode::Enter => {
                    let slot_on = *slot_on;
                    let outcome = *outcome;
                    let resolved_at = if input.text.trim().is_empty() {
                        None
                    } else {
                        match DateTime::parse_from_rfc3339(input.text.trim()) {
                            Ok(value) => Some(value.to_rfc3339()),
                            Err(_) => {
                                self.set_warning("resolution time must use RFC3339 form");
                                self.overlay =
                                    Some(OverlayState::RecurrenceHistory(Box::new(state)));
                                return Ok(());
                            }
                        }
                    };
                    self.submit_recurrence_history_correction(state, slot_on, outcome, resolved_at)
                        .await?;
                }
                _ => {
                    input.handle_key(key);
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                }
            },
        }
        Ok(())
    }

    pub(super) async fn handle_recurrence_history_mouse(
        &mut self,
        mut state: RecurrenceHistoryState,
        mouse: crossterm::event::MouseEvent,
        terminal_size: ratatui::layout::Size,
    ) -> Result<()> {
        use crossterm::event::{MouseButton, MouseEventKind};

        if !matches!(state.mode, RecurrenceHistoryMode::Browse) {
            state.mode = RecurrenceHistoryMode::Browse;
            self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
            return Ok(());
        }
        let view = crate::tui::overlay::RecurrenceHistoryView {
            page: state.page.clone(),
            selected: state.selected.clone(),
            mode: state.mode.clone(),
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => state.move_selection(1),
            MouseEventKind::ScrollUp => state.move_selection(-1),
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => {
                let Some(index) = crate::tui::ui::recurrence_history_entry_at(
                    &view,
                    terminal_size,
                    mouse.column,
                    mouse.row,
                ) else {
                    return Ok(());
                };
                state.selected = state
                    .page
                    .items
                    .get(index)
                    .map(recurrence_history_entry_key);
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right)) {
                    let action = state.selected_entry().and_then(|entry| {
                        if entry.openable && entry.task_id.is_some() {
                            Some(RecurrenceHistoryAction::OpenTask)
                        } else if recurrence_history_correction_block_reason(entry).is_none() {
                            Some(RecurrenceHistoryAction::Correct)
                        } else {
                            None
                        }
                    });
                    if let Some(action) = action {
                        return self.run_recurrence_history_action(state, action).await;
                    }
                    self.set_warning("this history entry has no available actions");
                }
            }
            _ => {}
        }
        self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
        Ok(())
    }

    async fn run_recurrence_history_action(
        &mut self,
        mut state: RecurrenceHistoryState,
        action: RecurrenceHistoryAction,
    ) -> Result<()> {
        if state.workspace_id != self.store.active_workspace.id {
            self.set_warning("recurring series target is unavailable");
            self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
            return Ok(());
        }
        let Some(entry) = state.selected_entry().cloned() else {
            self.set_warning("recurrence history has no selected entry");
            self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
            return Ok(());
        };
        match action {
            RecurrenceHistoryAction::OpenTask => {
                let Some(task_id) = entry.task_id.filter(|_| entry.openable) else {
                    self.set_warning("this history entry has no linked task");
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                    return Ok(());
                };
                let previous = self.store.view_state.clone();
                let Some(selected) = self.store.show_task_by_id(task_id).await? else {
                    self.store.restore_view_state(previous, None).await?;
                    self.set_warning("linked occurrence task is unavailable");
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                    return Ok(());
                };
                self.push_navigation_state(previous);
                self.list.select_task(Some(selected));
                self.detail = crate::tui::detail_session::DetailSession::open(0);
                self.overlay = None;
            }
            RecurrenceHistoryAction::Correct => {
                if let Some(reason) = recurrence_history_correction_block_reason(&entry) {
                    self.set_warning(reason);
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                    return Ok(());
                }
                let slot_on = NaiveDate::parse_from_str(
                    entry.slot_on.as_deref().unwrap_or_default(),
                    "%Y-%m-%d",
                )
                .context("invalid recurrence history slot")?;
                state.mode = RecurrenceHistoryMode::Outcome {
                    slot_on,
                    picker: PickerState::new(
                        PickerIntent::RecurrenceHistoryOutcome,
                        format!("Correct {slot_on}"),
                        vec![
                            PickerItem {
                                label: "Completed".to_string(),
                                value: "completed".to_string(),
                                selected: true,
                            },
                            PickerItem {
                                label: "Skipped".to_string(),
                                value: "skipped".to_string(),
                                selected: false,
                            },
                        ],
                        false,
                    ),
                };
                self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
            }
        }
        Ok(())
    }

    async fn submit_recurrence_history_correction(
        &mut self,
        state: RecurrenceHistoryState,
        slot_on: NaiveDate,
        outcome: RecurrenceOutcome,
        resolved_at: Option<String>,
    ) -> Result<()> {
        let preferred = Some(RecurrenceHistoryEntryKey::Slot(slot_on.to_string()));
        let fallback_index = state.selected_index().unwrap_or(0);
        match self
            .store
            .record_recurrence_outcome(&state.series_id, slot_on, outcome, resolved_at)
            .await
        {
            Ok(message) => {
                let offset = state.page.offset;
                self.set_success(message.message);
                self.load_recurrence_history_page(state, offset, preferred, fallback_index)
                    .await?;
            }
            Err(error) => {
                let Some(warning) = recurrence_correction_rejection(&error) else {
                    self.overlay = Some(OverlayState::RecurrenceHistory(Box::new(state)));
                    return Err(error);
                };
                let offset = state.page.offset;
                self.load_recurrence_history_page(state, offset, preferred, fallback_index)
                    .await?;
                self.set_warning(warning);
            }
        }
        Ok(())
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
}

use anyhow::{Context, Result, bail};
use aven_core::recurrence::{RecurrenceOutcome, RecurrenceSeriesState};
use chrono::{DateTime, NaiveDate};

use crate::tui::app::App;
use crate::tui::overlay::{OverlayState, TextIntent, TextPanelState};

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
    pub(super) async fn show_recurrence_history(&mut self) -> Result<()> {
        let Some(page) = self
            .store
            .recurrence_history_for_task(self.list.selected_task())
            .await?
        else {
            self.set_warning("selected task does not belong to a recurring series");
            return Ok(());
        };
        self.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            format!("Recurrence history: {}", page.series_ref),
            crate::tui::store::recurrence_history_lines(&page),
        )));
        Ok(())
    }

    pub(super) async fn begin_edit_recurrence_template(&mut self) -> Result<()> {
        let Some(item) = self
            .store
            .selected_task(self.list.selected_task())
            .cloned()
        else {
            self.set_warning("no selected recurring task");
            return Ok(());
        };
        let Some(detail) = self
            .store
            .recurrence_detail_for_task(self.list.selected_task())
            .await?
        else {
            self.set_warning("selected task does not belong to a recurring series");
            return Ok(());
        };
        self.authoring
            .begin_edit_recurrence_template(&detail, item.task.project_key.clone());
        self.begin_add_task_step();
        Ok(())
    }

    pub(super) fn begin_record_recurrence(&mut self) {
        let Some(summary) = self
            .store
            .selected_task(self.list.selected_task())
            .and_then(|item| item.recurrence.as_ref())
        else {
            self.set_warning("selected task does not belong to a recurring series");
            return;
        };
        self.overlay = Some(OverlayState::blank_text_input(
            TextIntent::RecordRecurrenceOutcome,
            format!("Record historical outcome: {}", summary.series_ref),
            "YYYY-MM-DD completed|skipped [RFC3339 time]",
        ));
    }

    pub(super) async fn submit_record_recurrence(&mut self, value: String) -> Result<()> {
        let Some(summary) = self
            .store
            .selected_task(self.list.selected_task())
            .and_then(|item| item.recurrence.as_ref())
            .cloned()
        else {
            self.set_warning("selected task does not belong to a recurring series");
            return Ok(());
        };
        let parsed = parse_historical_outcome(&value);
        let (slot, outcome, resolved_at) = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                self.set_warning(error.to_string());
                self.begin_record_recurrence();
                return Ok(());
            }
        };
        let message = self
            .store
            .record_recurrence_outcome(&summary.series_id, slot, outcome, resolved_at)
            .await?;
        self.list.select_task(message.selected);
        self.set_success(message.message);
        Ok(())
    }

    pub(super) async fn skip_recurrence(&mut self) -> Result<()> {
        let index = self.list.selected_task();
        let Some(message) = self.store.skip_recurrence(index).await? else {
            self.set_warning("selected task does not have a current recurring occurrence");
            return Ok(());
        };
        self.list.select_task(message.selected);
        self.set_success(message.message);
        Ok(())
    }

    pub(super) async fn pause_recurrence(&mut self) -> Result<()> {
        self.apply_recurrence_state(RecurrenceSeriesState::Paused)
            .await
    }

    pub(super) async fn resume_recurrence(&mut self) -> Result<()> {
        self.apply_recurrence_state(RecurrenceSeriesState::Active)
            .await
    }

    pub(super) async fn stop_recurrence(&mut self) -> Result<()> {
        self.apply_recurrence_state(RecurrenceSeriesState::Stopped)
            .await
    }

    async fn apply_recurrence_state(&mut self, state: RecurrenceSeriesState) -> Result<()> {
        let index = self.list.selected_task();
        let message = match state {
            RecurrenceSeriesState::Active => self.store.resume_recurrence(index).await?,
            RecurrenceSeriesState::Paused => self.store.pause_recurrence(index).await?,
            RecurrenceSeriesState::Stopped => self.store.stop_recurrence(index).await?,
        };
        let Some(message) = message else {
            self.set_warning("selected task does not belong to a recurring series");
            return Ok(());
        };
        self.list.select_task(message.selected);
        self.set_success(message.message);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

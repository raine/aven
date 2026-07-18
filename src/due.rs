use chrono::NaiveDate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueState {
    None,
    Future(i64),
    Today,
    Overdue(i64),
}

impl DueState {
    pub fn needs_action(self) -> bool {
        matches!(self, Self::Today | Self::Overdue(_))
    }

    #[cfg(test)]
    pub fn score(self) -> i32 {
        match self {
            Self::Today | Self::Overdue(_) => 40,
            Self::Future(days) if (1..=7).contains(&days) => (5 * (8 - days)) as i32,
            Self::None | Self::Future(_) => 0,
        }
    }
}

pub fn due_state(due_on: &str, today: NaiveDate) -> DueState {
    let Ok(due) = NaiveDate::parse_from_str(due_on, "%Y-%m-%d") else {
        return DueState::None;
    };
    let days = due.signed_duration_since(today).num_days();
    match days {
        0 => DueState::Today,
        1.. => DueState::Future(days),
        _ => DueState::Overdue(-days),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 16).unwrap()
    }

    #[test]
    fn classifies_calendar_deadlines() {
        assert_eq!(due_state("", today()), DueState::None);
        assert_eq!(due_state("2026-07-15", today()), DueState::Overdue(1));
        assert_eq!(due_state("2026-07-16", today()), DueState::Today);
        assert_eq!(due_state("2026-07-18", today()), DueState::Future(2));
    }

    #[test]
    fn scores_only_due_week_and_late_states() {
        assert_eq!(DueState::Future(8).score(), 0);
        assert_eq!(DueState::Future(7).score(), 5);
        assert_eq!(DueState::Future(1).score(), 35);
        assert_eq!(DueState::Today.score(), 40);
        assert_eq!(DueState::Overdue(30).score(), 40);
    }
}

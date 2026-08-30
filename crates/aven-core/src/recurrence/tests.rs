use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, Utc, Weekday};

use super::*;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn time(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
}

fn utc(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn schedule(
    rule: RecurrenceRule,
    zone: &str,
    start_on: NaiveDate,
    available_local_time: Option<NaiveTime>,
    due_policy: RecurrenceDuePolicy,
) -> RecurrenceSchedule {
    RecurrenceSchedule::new(
        rule,
        zone.parse().unwrap(),
        start_on,
        available_local_time,
        due_policy,
    )
}

#[test]
fn recurrence_series_ids_and_timezones_validate_ingress() {
    assert!("0123456789ABCDEF".parse::<RecurrenceSeriesId>().is_ok());
    assert!("0123456789ABCDE".parse::<RecurrenceSeriesId>().is_err());
    assert!("0123456789ABCDEI".parse::<RecurrenceSeriesId>().is_err());
    assert!("Europe/Stockholm".parse::<TimeZoneId>().is_ok());
    assert!("Europe/Not_A_Zone".parse::<TimeZoneId>().is_err());
    assert!(serde_json::from_str::<TimeZoneId>("\"local\"").is_err());
}

#[test]
fn rules_enforce_positive_intervals_and_weekly_only_weekdays() {
    for frequency in [
        RecurrenceFrequency::Daily,
        RecurrenceFrequency::Weekly,
        RecurrenceFrequency::Monthly,
        RecurrenceFrequency::Yearly,
    ] {
        let weekdays = if frequency == RecurrenceFrequency::Weekly {
            WeekdaySet::from_weekdays([Weekday::Mon])
        } else {
            WeekdaySet::default()
        };
        assert!(RecurrenceRule::new(frequency, 0, weekdays).is_err());
    }
    assert!(RecurrenceRule::weekly_on([]).is_err());
    assert!(RecurrenceRule::new(RecurrenceFrequency::Weekly, 1, WeekdaySet::default()).is_err());
    for frequency in [
        RecurrenceFrequency::Daily,
        RecurrenceFrequency::Monthly,
        RecurrenceFrequency::Yearly,
    ] {
        assert!(
            RecurrenceRule::new(frequency, 1, WeekdaySet::from_weekdays([Weekday::Mon]),).is_err()
        );
    }
    assert!(RecurrenceRule::every_n_days(2).is_ok());
    assert!(RecurrenceRule::every_n_months(2).is_ok());
    assert!(RecurrenceRule::every_n_years(2).is_ok());
    assert!(
        serde_json::from_str::<RecurrenceRule>(
            r#"{"frequency":"weekly","interval":1,"weekdays":"fri,mon"}"#,
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<RecurrenceRule>(
            r#"{"frequency":"weekly","interval":1,"weekdays":"mon,mon"}"#,
        )
        .is_err()
    );
}

#[test]
fn daily_and_weekday_iteration_crosses_leap_dates() {
    let daily = schedule(
        RecurrenceRule::daily(),
        "UTC",
        date(2028, 2, 28),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        daily
            .slots_on_or_after(date(2028, 2, 28))
            .take(3)
            .collect::<Vec<_>>(),
        vec![date(2028, 2, 28), date(2028, 2, 29), date(2028, 3, 1)]
    );

    let weekdays = schedule(
        RecurrenceRule::weekdays(),
        "UTC",
        date(2026, 7, 17),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        weekdays
            .slots_on_or_after(date(2026, 7, 17))
            .take(4)
            .collect::<Vec<_>>(),
        vec![
            date(2026, 7, 17),
            date(2026, 7, 20),
            date(2026, 7, 21),
            date(2026, 7, 22),
        ]
    );
}

#[test]
fn interval_recurrences_derive_every_slot_from_the_anchor() {
    let daily = schedule(
        RecurrenceRule::every_n_days(3).unwrap(),
        "UTC",
        date(2026, 8, 10),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        daily
            .slots_on_or_after(daily.start_on)
            .take(4)
            .collect::<Vec<_>>(),
        vec![
            date(2026, 8, 10),
            date(2026, 8, 13),
            date(2026, 8, 16),
            date(2026, 8, 19),
        ]
    );

    let monthly = schedule(
        RecurrenceRule::every_n_months(3).unwrap(),
        "UTC",
        date(2027, 1, 31),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        monthly
            .slots_on_or_after(monthly.start_on)
            .take(5)
            .collect::<Vec<_>>(),
        vec![
            date(2027, 1, 31),
            date(2027, 4, 30),
            date(2027, 7, 31),
            date(2027, 10, 31),
            date(2028, 1, 31),
        ]
    );

    let yearly = schedule(
        RecurrenceRule::yearly(),
        "UTC",
        date(2028, 2, 29),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        yearly
            .slots_on_or_after(yearly.start_on)
            .take(5)
            .collect::<Vec<_>>(),
        vec![
            date(2028, 2, 29),
            date(2029, 2, 28),
            date(2030, 2, 28),
            date(2031, 2, 28),
            date(2032, 2, 29),
        ]
    );
    let every_two_years = RecurrenceRule::every_n_years(2).unwrap();
    assert_eq!(
        schedule(
            every_two_years,
            "UTC",
            date(2028, 2, 29),
            None,
            RecurrenceDuePolicy::None
        )
        .slots_on_or_after(date(2029, 1, 1))
        .take(3)
        .collect::<Vec<_>>(),
        vec![date(2030, 2, 28), date(2032, 2, 29), date(2034, 2, 28)]
    );
    let json = serde_json::to_string(&every_two_years).unwrap();
    assert_eq!(json, r#"{"frequency":"yearly","interval":2,"weekdays":""}"#);
    assert_eq!(
        serde_json::from_str::<RecurrenceRule>(&json).unwrap(),
        every_two_years
    );
}

#[test]
fn direct_navigation_matches_slot_scan_oracles() {
    let cases = [
        (RecurrenceRule::every_n_days(3).unwrap(), date(2026, 8, 10)),
        (
            RecurrenceRule::every_n_weeks_on(3, [Weekday::Mon, Weekday::Thu]).unwrap(),
            date(2026, 8, 12),
        ),
        (
            RecurrenceRule::every_n_months(3).unwrap(),
            date(2027, 1, 31),
        ),
        (RecurrenceRule::every_n_years(2).unwrap(), date(2028, 2, 29)),
    ];
    for (rule, start_on) in cases {
        for offset in 0..500 {
            let query = start_on
                .checked_add_days(chrono::Days::new(offset))
                .unwrap();
            let expected_after = (offset..=900)
                .map(|candidate| {
                    start_on
                        .checked_add_days(chrono::Days::new(candidate))
                        .unwrap()
                })
                .find(|candidate| is_slot(&rule, start_on, *candidate));
            assert_eq!(
                super::schedule::slot_on_or_after(&rule, start_on, query),
                expected_after
            );
            let expected_before = (0..=offset)
                .rev()
                .map(|candidate| {
                    start_on
                        .checked_add_days(chrono::Days::new(candidate))
                        .unwrap()
                })
                .find(|candidate| is_slot(&rule, start_on, *candidate));
            assert_eq!(
                super::schedule::slot_on_or_before(&rule, start_on, query),
                expected_before
            );
        }
    }
}

#[test]
fn direct_navigation_handles_partial_weeks_large_intervals_and_overflow() {
    let start_on = date(2026, 7, 15);
    let rule = RecurrenceRule::every_n_weeks_on(u32::MAX, [Weekday::Mon, Weekday::Thu]).unwrap();
    assert_eq!(
        schedule(rule, "UTC", start_on, None, RecurrenceDuePolicy::None)
            .slots_on_or_after(start_on)
            .next(),
        Some(date(2026, 7, 16))
    );
    assert!(!is_slot(&rule, start_on, date(2026, 7, 13)));
    assert_eq!(next_slot_after(&rule, start_on, date(2026, 7, 16)), None);

    let daily = RecurrenceRule::every_n_days(u32::MAX).unwrap();
    assert_eq!(
        next_slot_after(&daily, NaiveDate::MAX, NaiveDate::MAX),
        None
    );
}

#[test]
fn monthly_iteration_clamps_to_each_month_without_drifting() {
    let monthly = schedule(
        RecurrenceRule::monthly(),
        "UTC",
        date(2027, 1, 31),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        monthly
            .slots_on_or_after(monthly.start_on)
            .take(4)
            .collect::<Vec<_>>(),
        vec![
            date(2027, 1, 31),
            date(2027, 2, 28),
            date(2027, 3, 31),
            date(2027, 4, 30),
        ]
    );

    let leap_year = schedule(
        RecurrenceRule::monthly(),
        "UTC",
        date(2028, 1, 31),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        leap_year
            .slots_on_or_after(date(2028, 2, 1))
            .take(2)
            .collect::<Vec<_>>(),
        vec![date(2028, 2, 29), date(2028, 3, 31)]
    );
    assert!(is_slot(
        &leap_year.rule,
        leap_year.start_on,
        date(2028, 2, 29)
    ));
    assert!(!is_slot(
        &leap_year.rule,
        leap_year.start_on,
        date(2028, 2, 28)
    ));
    assert_eq!(
        next_slot_after(&leap_year.rule, leap_year.start_on, date(2028, 1, 31)),
        Some(date(2028, 2, 29))
    );
    assert_eq!(
        live_slot_on(
            &leap_year.rule,
            leap_year.start_on,
            utc("2028-04-01T12:00:00Z"),
            &leap_year.timezone,
        ),
        Some(date(2028, 3, 31))
    );

    let twenty_eighth = schedule(
        RecurrenceRule::monthly(),
        "UTC",
        date(2027, 2, 28),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        twenty_eighth
            .slots_on_or_after(twenty_eighth.start_on)
            .take(3)
            .collect::<Vec<_>>(),
        vec![date(2027, 2, 28), date(2027, 3, 28), date(2027, 4, 28),]
    );
}

#[test]
fn monthly_iteration_uses_the_original_day_across_year_boundaries() {
    let monthly = schedule(
        RecurrenceRule::monthly(),
        "UTC",
        date(2027, 12, 30),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        monthly
            .slots_on_or_after(date(2027, 12, 31))
            .take(3)
            .collect::<Vec<_>>(),
        vec![date(2028, 1, 30), date(2028, 2, 29), date(2028, 3, 30),]
    );
}

#[test]
fn weekly_sets_use_monday_anchored_multi_week_intervals() {
    let rule = RecurrenceRule::every_n_weeks_on(2, [Weekday::Mon, Weekday::Thu]).unwrap();
    let schedule = schedule(
        rule,
        "UTC",
        date(2026, 7, 15),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        schedule
            .slots_on_or_after(date(2026, 7, 1))
            .take(6)
            .collect::<Vec<_>>(),
        vec![
            date(2026, 7, 16),
            date(2026, 7, 27),
            date(2026, 7, 30),
            date(2026, 8, 10),
            date(2026, 8, 13),
            date(2026, 8, 24),
        ]
    );
    assert!(!is_slot(&rule, schedule.start_on, date(2026, 7, 13)));
    assert!(is_slot(&rule, schedule.start_on, date(2026, 7, 16)));
}

#[test]
fn weekly_shorthand_uses_the_start_weekday() {
    let start_on = date(2026, 7, 15);
    let rule = RecurrenceRule::weekly(start_on.weekday());
    assert_eq!(
        RecurrenceSchedule::new(
            rule,
            "UTC".parse().unwrap(),
            start_on,
            None,
            RecurrenceDuePolicy::SameDay,
        )
        .slots_on_or_after(start_on)
        .take(3)
        .collect::<Vec<_>>(),
        vec![date(2026, 7, 15), date(2026, 7, 22), date(2026, 7, 29)]
    );
}

#[test]
fn weekly_live_slots_end_at_the_next_scheduled_boundary() {
    let schedule = schedule(
        RecurrenceRule::weekly(Weekday::Fri),
        "Europe/Stockholm",
        date(2026, 7, 3),
        Some(time(9, 0)),
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        live_slot_on(
            &schedule.rule,
            schedule.start_on,
            utc("2026-07-09T21:59:59Z"),
            &schedule.timezone,
        ),
        Some(date(2026, 7, 3))
    );
    assert_eq!(
        live_slot_on(
            &schedule.rule,
            schedule.start_on,
            utc("2026-07-09T22:00:00Z"),
            &schedule.timezone,
        ),
        Some(date(2026, 7, 10))
    );
    assert_eq!(
        slot_cutoff(&schedule, date(2026, 7, 3)).unwrap(),
        utc("2026-07-09T22:00:00Z")
    );
}

#[test]
fn midnight_transition_preserves_live_slot_boundaries() {
    let schedule = schedule(
        RecurrenceRule::daily(),
        "America/Sao_Paulo",
        date(2018, 11, 3),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        live_slot_on(
            &schedule.rule,
            schedule.start_on,
            utc("2018-11-04T02:59:59Z"),
            &schedule.timezone,
        ),
        Some(date(2018, 11, 3))
    );
    assert_eq!(
        live_slot_on(
            &schedule.rule,
            schedule.start_on,
            utc("2018-11-04T03:00:00Z"),
            &schedule.timezone,
        ),
        Some(date(2018, 11, 4))
    );
}

#[test]
fn nonexistent_availability_advances_to_the_first_valid_instant() {
    let schedule = schedule(
        RecurrenceRule::daily(),
        "Europe/Stockholm",
        date(2026, 3, 29),
        Some(time(2, 30)),
        RecurrenceDuePolicy::SameDay,
    );
    let slot = slot_values(&schedule, date(2026, 3, 29)).unwrap();
    assert_eq!(slot.boundary_at, "2026-03-28T23:00:00Z");
    assert_eq!(slot.available_at, "2026-03-29T01:00:00Z");
    assert_eq!(slot.due_on.as_deref(), Some("2026-03-29"));
}

#[test]
fn repeated_availability_uses_the_earlier_instant() {
    let schedule = schedule(
        RecurrenceRule::daily(),
        "Europe/Stockholm",
        date(2026, 10, 25),
        Some(time(2, 30)),
        RecurrenceDuePolicy::None,
    );
    let slot = slot_values(&schedule, date(2026, 10, 25)).unwrap();
    assert_eq!(slot.available_at, "2026-10-25T00:30:00Z");
    assert_eq!(slot.due_on, None);
}

#[test]
fn slot_expansion_rejects_dates_outside_the_rule() {
    let schedule = schedule(
        RecurrenceRule::weekly(Weekday::Mon),
        "UTC",
        date(2026, 7, 20),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    assert_eq!(
        slot_values(&schedule, date(2026, 7, 21)).unwrap_err(),
        RecurrenceScheduleError::NotScheduledSlot
    );
    assert_eq!(
        live_slot_on(
            &schedule.rule,
            schedule.start_on,
            utc("2026-07-19T23:59:59Z"),
            &schedule.timezone,
        ),
        None
    );
}

#[test]
fn independent_replicas_derive_byte_identical_occurrence_values() {
    let workspace_id: crate::ids::WorkspaceId = "0123456789ABCDEF".parse().unwrap();
    let series_id: RecurrenceSeriesId = "ZYXWVTSRQPNMKJHG".parse().unwrap();
    let schedule = schedule(
        RecurrenceRule::weekly_on([Weekday::Mon, Weekday::Wed, Weekday::Fri]).unwrap(),
        "America/New_York",
        date(2028, 2, 28),
        Some(time(9, 15)),
        RecurrenceDuePolicy::SameDay,
    );

    let replica_a =
        derive_occurrence_identity(&workspace_id, &series_id, &schedule, date(2028, 3, 1)).unwrap();
    let replica_b =
        derive_occurrence_identity(&workspace_id, &series_id, &schedule, date(2028, 3, 1)).unwrap();
    assert_eq!(
        serde_json::to_vec(&replica_a).unwrap(),
        serde_json::to_vec(&replica_b).unwrap()
    );
    assert_eq!(replica_a.task_id.as_str(), "5YRVRHYJ8XWCX9FE");
    assert_eq!(replica_a.task_change_id, "ASXSYZE398NVZWPE");
    assert_eq!(replica_a.occurrence_change_id, "3TTRAPANPRCKAHRY");
    assert_eq!(replica_a.field_version_seeds.task, "RJT435ECKYW1M2KR");
    assert_eq!(replica_a.field_version_seeds.occurrence, "FVHPMBRNJJ4PBV1S");
    assert_ne!(replica_a.task_id.as_str(), replica_a.task_change_id);
    assert_ne!(replica_a.task_change_id, replica_a.occurrence_change_id);
    assert_ne!(
        replica_a.field_version_seeds.task,
        replica_a.field_version_seeds.occurrence
    );
    assert_eq!(replica_a.created_at, "2028-03-01T05:00:00Z");
    assert_eq!(replica_a.updated_at, replica_a.created_at);
    assert_eq!(replica_a.occurrence_link.projected_at, replica_a.created_at);
}

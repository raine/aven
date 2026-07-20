mod common;

use aven_core::db::Database;
use aven_core::operations::RecurrenceSeriesDraft;
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule, TimeZoneId};
use chrono::{Duration, Utc};
use common::{TestEnv, contains_all, contains_none, fail, ok};
use sqlx::{Connection, SqliteConnection};

fn field(output: &str, name: &str) -> String {
    output
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{name}=")))
        .expect("named output field")
        .trim_matches('"')
        .to_string()
}

fn add_daily(env: &TestEnv, db: &std::path::Path, title: &str) -> (String, String) {
    let today = Utc::now().date_naive().to_string();
    let output = ok(env.aven(
        db,
        [
            "add",
            title,
            "--repeat",
            "daily",
            "--repeat-due",
            "same-day",
            "--time-zone",
            "UTC",
            "--repeat-start-on",
            &today,
        ],
    ));
    let series_ref = output.split_whitespace().nth(1).unwrap().to_string();
    let occurrence_ref = field(&output, "occurrence");
    (series_ref, occurrence_ref)
}

#[test]
fn add_parses_fixed_rules_and_rejects_ambiguous_scheduling() {
    let env = TestEnv::new();
    let db = env.db("recurrence-add.sqlite");
    let today = Utc::now().date_naive().to_string();

    for (index, rule) in [
        "daily",
        "weekdays",
        "weekly",
        "weekly on mon,wed,fri",
        "every 2 weeks on tue",
        "every 3 weeks on mon,thu",
    ]
    .into_iter()
    .enumerate()
    {
        let output = ok(env.aven(
            &db,
            [
                "add",
                &format!("series {index}"),
                "--repeat",
                rule,
                "--repeat-at",
                "09:00",
                "--repeat-due",
                "none",
                "--time-zone",
                "Europe/Stockholm",
                "--repeat-start-on",
                &today,
            ],
        ));
        contains_all(&output, &["created RCR-", "occurrence=", "status=todo"]);
    }

    for rule in [
        "every 3 days",
        "weekly on monday",
        "weekly on fri,mon",
        "every two weeks on tue",
    ] {
        let error = fail(env.aven(
            &db,
            [
                "add",
                "invalid series",
                "--repeat",
                rule,
                "--time-zone",
                "UTC",
            ],
        ));
        assert!(
            error.contains("invalid-repeat-rule")
                || error.contains("weekday set")
                || error.contains("invalid-repeat-interval"),
            "{error}"
        );
    }

    let error = fail(env.aven(
        &db,
        [
            "add",
            "mixed scheduling",
            "--repeat",
            "daily",
            "--available-at",
            "tomorrow",
            "--due",
            "tomorrow",
            "--time-zone",
            "UTC",
        ],
    ));
    contains_all(
        &error,
        &[
            "recurrence-absolute-time-conflict",
            "--repeat-at",
            "--repeat-due",
        ],
    );

    let error = fail(env.aven(&db, ["add", "orphan flags", "--repeat-at", "09:00"]));
    contains_all(&error, &["recurrence-flags-require-repeat", "--repeat"]);
}

#[test]
fn completion_groups_list_and_search_while_expansion_keeps_occurrence_refs() {
    let env = TestEnv::new();
    let db = env.db("recurrence-grouping.sqlite");
    let (series_ref, occurrence_ref) = add_daily(&env, &db, "Daily journal");

    let context: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["context", &occurrence_ref, "--json"]))).unwrap();
    assert_eq!(context["recurrence"]["series_ref"], series_ref);
    assert_eq!(
        context["recurrence"]["slot_on"],
        Utc::now().date_naive().to_string()
    );

    ok(env.aven(&db, ["edit", &occurrence_ref, "--status", "done"]));

    let grouped = ok(env.aven(&db, ["list", "--status", "done"]));
    contains_all(
        &grouped,
        &[&series_ref, "completed=1", "title=\"Daily journal\""],
    );
    contains_none(&grouped, &[&occurrence_ref]);

    let expanded = ok(env.aven(&db, ["list", "--status", "done", "--expand-recurring"]));
    contains_all(
        &expanded,
        &[
            &occurrence_ref,
            &format!("series={series_ref}"),
            "outcome=completed",
        ],
    );

    let search = ok(env.aven(&db, ["search", "Daily journal"]));
    contains_all(&search, &[&series_ref, "completed=1"]);
    let search_expanded = ok(env.aven(&db, ["search", "Daily journal", "--expand-recurring"]));
    contains_all(&search_expanded, &[&occurrence_ref, "match=title"]);

    let list_json: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["list", "--status", "done", "--json"]))).unwrap();
    assert_eq!(list_json[0]["ref"], series_ref);
    assert_eq!(list_json[0]["recurrence_group"]["series_ref"], series_ref);
    assert_eq!(list_json[0]["recurrence_group"]["completed"], 1);
}

#[test]
fn edit_changes_future_template_without_rewriting_current_occurrence() {
    let env = TestEnv::new();
    let db = env.db("recurrence-edit.sqlite");
    let (series_ref, occurrence_ref) = add_daily(&env, &db, "Daily journal");
    ok(env.aven(&db, ["label", "create", "review"]));

    let output = ok(env.aven(
        &db,
        [
            "recur",
            "edit",
            &occurrence_ref,
            "--title",
            "Future journal",
            "--priority",
            "high",
            "--status",
            "active",
            "--repeat-at",
            "10:30",
            "--repeat-due",
            "none",
            "--label",
            "review",
        ],
    ));
    contains_all(
        &output,
        &[&series_ref, "changed=yes", "title=\"Future journal\""],
    );

    let current = ok(env.aven(&db, ["show", &occurrence_ref]));
    contains_all(&current, &["priority=none", "title=\"Daily journal\""]);

    ok(env.aven(&db, ["edit", &occurrence_ref, "--status", "done"]));
    let shown: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["recur", "show", &series_ref, "--json"]))).unwrap();
    assert_eq!(shown["version"], 1);
    assert_eq!(shown["series"]["title"], "Future journal");
    assert_eq!(shown["series"]["priority"], "high");
    assert_eq!(shown["series"]["initial_status"], "active");
    assert_eq!(shown["series"]["available_at"], "10:30");
    assert_eq!(shown["series"]["due"], "none");
    assert_eq!(shown["series"]["labels"][0], "review");

    let successor_ref = shown["series"]["current_task_ref"].as_str().unwrap();
    let successor = ok(env.aven(&db, ["show", successor_ref]));
    contains_all(
        &successor,
        &["status=active", "priority=high", "title=\"Future journal\""],
    );
}

#[test]
fn pause_resume_stop_and_delete_follow_recurrence_lifecycle() {
    let env = TestEnv::new();
    let db = env.db("recurrence-lifecycle.sqlite");
    let (series_ref, occurrence_ref) = add_daily(&env, &db, "Equipment check");

    let delete_error = fail(env.aven(&db, ["delete", &occurrence_ref]));
    contains_all(
        &delete_error,
        &["recurrence-current-delete", "skip, pause, or stop"],
    );

    let paused = ok(env.aven(&db, ["recur", "pause", &occurrence_ref]));
    contains_all(&paused, &["paused", &series_ref]);
    let list = ok(env.aven(&db, ["list"]));
    contains_none(&list, &["Equipment check"]);
    let direct = ok(env.aven(&db, ["show", &occurrence_ref]));
    contains_all(
        &direct,
        &["series_state=paused", "title=\"Equipment check\""],
    );

    let resumed = ok(env.aven(&db, ["recur", "resume", &series_ref]));
    contains_all(&resumed, &["active", &series_ref]);
    contains_all(&ok(env.aven(&db, ["list"])), &["Equipment check"]);

    let stopped = ok(env.aven(&db, ["recur", "stop", &series_ref]));
    contains_all(&stopped, &["stopped", &series_ref]);
    contains_all(&ok(env.aven(&db, ["list"])), &["series_state=stopped"]);

    ok(env.aven(&db, ["edit", &occurrence_ref, "--status", "done"]));
    let detail = ok(env.aven(&db, ["recur", "show", &series_ref]));
    assert!(detail.contains("state=stopped"), "{detail}");
    assert!(!detail.contains(" current="), "{detail}");

    ok(env.aven(&db, ["delete", &occurrence_ref]));
    contains_all(
        &ok(env.aven(&db, ["show", &occurrence_ref])),
        &["deleted=yes"],
    );
    ok(env.aven(&db, ["restore", &occurrence_ref]));
    contains_none(
        &ok(env.aven(&db, ["show", &occurrence_ref])),
        &["deleted=yes"],
    );
}

#[test]
fn history_combines_archived_misses_derived_misses_and_taskless_corrections() {
    let env = TestEnv::new();
    let db = env.db("recurrence-history.sqlite");
    ok(env.aven(&db, ["workspace", "list"]));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (series_ref, archived_task_id, correction_slot) = runtime.block_on(async {
        let database = Database::open(&db).await.unwrap();
        let workspace = database.resolve_workspace("default").await.unwrap();
        let created_at = Utc::now() - Duration::days(6);
        let start_on = created_at.date_naive();
        let result = database
            .create_recurrence_series_at(
                &workspace,
                RecurrenceSeriesDraft {
                    title: "Six days behind".to_string(),
                    description: String::new(),
                    project: "default".to_string(),
                    priority: "none".to_string(),
                    initial_status: "todo".to_string(),
                    labels: vec![],
                    schedule: RecurrenceSchedule::new(
                        RecurrenceRule::daily(),
                        "UTC".parse::<TimeZoneId>().unwrap(),
                        start_on,
                        None,
                        RecurrenceDuePolicy::SameDay,
                    ),
                },
                created_at,
            )
            .await
            .unwrap();
        (
            result.series_ref,
            result.task.id.to_string(),
            start_on.succ_opt().unwrap(),
        )
    });

    let history = ok(env.aven(&db, ["recur", "history", &series_ref]));
    contains_all(
        &history,
        &["missed", "archived_projection=yes", "openable=no"],
    );
    assert!(history.matches("missed slot=").count() >= 5, "{history}");

    let corrected_at = format!("{}T18:30:00Z", correction_slot);
    let record = ok(env.aven(
        &db,
        [
            "recur",
            "record",
            &series_ref,
            "--slot",
            &correction_slot.to_string(),
            "--outcome",
            "completed",
            "--at",
            &corrected_at,
        ],
    ));
    contains_all(&record, &["outcome=completed", "taskless=yes"]);

    let history_json: serde_json::Value = serde_json::from_str(&ok(
        env.aven(&db, ["recur", "history", &series_ref, "--json"])
    ))
    .unwrap();
    assert_eq!(history_json["version"], 1);
    let entries = history_json["history"]["entries"].as_array().unwrap();
    assert!(entries.iter().any(|entry| {
        entry["slot_on"] == correction_slot.to_string()
            && entry["outcome"] == "completed"
            && entry["corrected"] == true
            && entry["openable"] == false
            && entry["task_id"].is_null()
    }));
    assert!(entries.iter().any(|entry| {
        entry["task_id"] == archived_task_id
            && entry["outcome"] == "missed"
            && entry["archived_projection"] == true
            && entry["openable"] == true
    }));
}

#[test]
fn series_show_renders_lifecycle_conflicts_without_hiding_current_work() {
    let env = TestEnv::new();
    let db = env.db("recurrence-conflict.sqlite");
    let (series_ref, occurrence_ref) = add_daily(&env, &db, "Conflicted cadence");
    let shown: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["recur", "show", &series_ref, "--json"]))).unwrap();
    let series_id = shown["series"]["id"].as_str().unwrap().to_string();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut conn = SqliteConnection::connect(&format!("sqlite://{}", db.display()))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO conflicts(
                workspace_id, entity_type, entity_id, task_id, field, base_version,
                local_value, remote_value, local_change_id, remote_change_id,
                variant_a, variant_b, created_at, resolved
             ) VALUES (
                '0000000000000000', 'recurrence_series', ?, '', 'state', NULL,
                'active', 'paused', NULL, 'REMOTECHANGE0001',
                'local', 'remote', '2026-07-20T00:00:00Z', 0
             )",
        )
        .bind(&series_id)
        .execute(&mut conn)
        .await
        .unwrap();
    });

    let text = ok(env.aven(&db, ["recur", "show", &series_ref]));
    contains_all(
        &text,
        &[
            &format!("conflict {series_ref} field=state"),
            "lifecycle_blocked=yes",
            "variant local value=\"active\"",
            "variant remote value=\"paused\"",
        ],
    );
    contains_all(
        &ok(env.aven(&db, ["show", &occurrence_ref])),
        &["Conflicted cadence"],
    );

    let json: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["recur", "show", &series_ref, "--json"]))).unwrap();
    assert_eq!(json["series"]["lifecycle_conflicts"][0]["field"], "state");
    assert_eq!(
        json["series"]["lifecycle_conflicts"][0]["variants"][1]["value"],
        "paused"
    );
}

#[test]
fn ordinary_tasks_keep_compatible_behavior_and_json_shape() {
    let env = TestEnv::new();
    let db = env.db("recurrence-compat.sqlite");
    let created = ok(env.aven(&db, ["add", "Ordinary task", "--priority", "medium"]));
    contains_all(&created, &["status=inbox", "priority=medium"]);
    let task_ref = created.split_whitespace().nth(1).unwrap();

    let json: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", task_ref, "--json"]))).unwrap();
    assert!(json["recurrence"].is_null());
    assert!(json["recurrence_group"].is_null());

    ok(env.aven(&db, ["edit", task_ref, "--status", "done"]));
    ok(env.aven(&db, ["edit", task_ref, "--status", "todo"]));
    ok(env.aven(&db, ["delete", task_ref]));
    ok(env.aven(&db, ["restore", task_ref]));
}

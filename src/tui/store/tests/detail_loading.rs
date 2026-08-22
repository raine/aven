use super::*;

#[tokio::test]
async fn exact_task_load_ignores_active_view_filters() {
    let mut store = test_store().await;
    let (task_id, index) = create_selected_task(&mut store, "Filtered detail").await;
    store.update_status(Some(index), "todo").await.unwrap();
    store.view_state.query = TaskQuery::Inbox;
    store.refresh(None).await.unwrap();
    assert!(store.tasks.iter().all(|item| item.task.id != task_id));

    let item = store.load_task_item(&task_id).await.unwrap().unwrap();

    assert_eq!(item.task.id, task_id);
    assert_eq!(item.task.status.as_str(), "todo");
}

#[tokio::test]
async fn task_full_report_renders_shareable_markdown() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    store
        .create_label("customer-facing".to_string())
        .await
        .unwrap();
    let (_, selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "Share task | safely".to_string(),
                description: "Explain **why**.\n\n- Keep authored Markdown".to_string(),
                project: None,
                status: "active".to_string(),
                priority: "high".to_string(),
                source: TaskSource::Tui,
                labels: vec!["customer-facing".to_string()],
                available_at: Some("2026-08-03T09:00:00Z".to_string()),
                due_on: Some("2026-08-10".to_string()),
                is_epic: false,
            },
            None,
        )
        .await
        .unwrap();
    let task_id = store.tasks[selected.unwrap()].task.id.clone();
    store
        .add_note_to_task(&task_id, "Decision: use `y m`.".to_string())
        .await
        .unwrap();
    seed_title_conflict(&pool, &task_id).await;

    let report = store.task_full_report(&task_id).await.unwrap().unwrap();
    let markdown = crate::task_render::task_markdown(&report);

    assert!(markdown.starts_with("# Task "));
    assert!(markdown.contains("**Active**"));
    assert!(markdown.contains(" · Project **"));
    assert!(markdown.contains(" · **High priority**"));
    assert!(markdown.contains("**Labels:** `customer-facing`"));
    assert!(markdown.contains("**Available:** 2026-08-03 09:00 UTC · **Due:** 2026-08-10"));
    assert!(markdown.contains("## Description\n\nExplain **why**."));
    assert!(markdown.contains("## Notes"));
    assert!(markdown.contains("Decision: use `y m`."));
    assert!(markdown.contains("> **Warning:** This task has 1 unresolved sync conflict."));
    assert!(markdown.contains("## Unresolved sync conflicts"));
    assert!(markdown.contains("**Local** (`a`): `local title`"));
    assert!(markdown.contains("**Remote** (`b`): `remote title`"));
    assert!(markdown.contains(&format!("Aven · task `{task_id}`")));
    assert!(markdown.ends_with(
        "\n\n<sub>Created with <a href=\"https://github.com/raine/aven\">aven</a></sub>\n"
    ));
    assert!(markdown.ends_with('\n'));
}

#[tokio::test]
async fn task_markdown_omits_empty_optional_sections() {
    let mut store = test_store().await;
    let (task_id, _) = create_selected_task(&mut store, "Minimal share").await;

    let report = store.task_full_report(&task_id).await.unwrap().unwrap();
    let markdown = crate::task_render::task_markdown(&report);

    assert!(markdown.starts_with("# Minimal share\n\n"));
    assert!(!markdown.contains("| Field | Value |"));
    assert!(!markdown.contains("None priority"));
    assert!(!markdown.contains(" · updated "));
    for omitted in [
        "## Description",
        "## Epic",
        "## Relationships",
        "## Recurrence",
        "## Attachments",
        "## Notes",
        "## Unresolved sync conflicts",
    ] {
        assert!(!markdown.contains(omitted), "unexpected section {omitted}");
    }
}

#[tokio::test]
async fn exact_task_load_returns_none_for_missing_id() {
    let store = test_store().await;
    let missing = crate::test_support::task_id("missing-detail-task");

    assert!(store.load_task_item(&missing).await.unwrap().is_none());
}

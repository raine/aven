use super::*;
use crate::query::test_support::*;
use crate::query::{list_project_items_in_workspace, sidebar_counts_for_scope_in_workspace};

#[tokio::test]
async fn queue_sort_orders_status_then_priority_then_created_at() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    for (id, title, status, priority, created_at) in [
        ("0000000000000001", "done urgent", "done", "urgent", "001"),
        ("0000000000000002", "inbox urgent", "inbox", "urgent", "002"),
        ("0000000000000003", "active low", "active", "low", "003"),
        ("0000000000000004", "todo urgent", "todo", "urgent", "004"),
        (
            "0000000000000005",
            "active urgent",
            "active",
            "urgent",
            "005",
        ),
    ] {
        insert_test_task(&mut conn, id, title, status, priority, created_at).await;
    }

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    assert_eq!(
        listed_titles(&items),
        ["active urgent", "todo urgent", "active low", "inbox urgent"]
    );
}

#[tokio::test]
async fn empty_task_id_restriction_matches_nothing() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "0000000000000001",
        "visible task",
        "todo",
        "none",
        "001",
    )
    .await;

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            task_ids: TaskIdFilter::Only(Vec::new()),
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert!(items.is_empty());
}

#[tokio::test]
async fn queue_view_hides_done_and_canceled_tasks() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    for (id, title, status) in [
        ("0000000000000111", "todo task", "todo"),
        ("0000000000000112", "done task", "done"),
        ("0000000000000113", "canceled task", "canceled"),
    ] {
        insert_test_task(&mut conn, id, title, status, "none", "001").await;
    }

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    assert_eq!(listed_titles(&items), ["todo task"]);

    let counts = sidebar_counts_for_scope_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(counts.open, 1);
    assert_eq!(counts.done, 2);
}

#[tokio::test]
async fn priority_sort_orders_priority_then_created_at() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    for (id, title, priority, created_at) in [
        ("0000000000000101", "none old", "none", "001"),
        ("0000000000000102", "urgent", "urgent", "002"),
        ("0000000000000103", "high", "high", "003"),
        ("0000000000000104", "none new", "none", "004"),
    ] {
        insert_test_task(&mut conn, id, title, "todo", priority, created_at).await;
    }

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters::default(),
        TaskQueryMode::Flat,
        TaskSort::Priority,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    assert_eq!(
        listed_titles(&items),
        ["urgent", "high", "none new", "none old"]
    );
}

#[tokio::test]
async fn created_sort_respects_direction() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    for (id, title, created_at) in [
        ("0000000000000201", "first", "001"),
        ("0000000000000202", "second", "002"),
        ("0000000000000203", "third", "002"),
    ] {
        insert_test_task(&mut conn, id, title, "todo", "none", created_at).await;
    }

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters::default(),
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Desc,
    )
    .await
    .unwrap();
    assert_eq!(listed_titles(&items), ["third", "second", "first"]);

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters::default(),
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    assert_eq!(listed_titles(&items), ["first", "second", "third"]);
}

#[tokio::test]
async fn conflicts_only_filter_returns_unresolved_conflicts() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    for (id, title) in [
        ("0000000000000011", "conflicted"),
        ("0000000000000012", "clean"),
    ] {
        insert_test_task(&mut conn, id, title, "todo", "none", "001").await;
    }

    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES ('0000000000000011', 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(conn.as_mut())
    .await
    .unwrap();

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            conflicts_only: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].task.title, "conflicted");
    assert!(items[0].has_conflict);
}

#[tokio::test]
async fn list_items_include_labels_and_unresolved_conflict_flags() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "0000000000000301",
        "labeled",
        "todo",
        "none",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "0000000000000302",
        "resolved",
        "todo",
        "none",
        "002",
    )
    .await;
    insert_test_task(
        &mut conn,
        "0000000000000303",
        "plain",
        "todo",
        "none",
        "003",
    )
    .await;

    insert_test_label(&mut conn, "0000000000000301", "zeta").await;
    insert_test_label(&mut conn, "0000000000000301", "alpha").await;
    insert_test_conflict(&mut conn, "0000000000000301", false).await;
    insert_test_conflict(&mut conn, "0000000000000302", true).await;
    sqlx::query(
        "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, '0000000000000302', '0000000000000301', '003')",
    )
    .bind(crate::workspaces::DEFAULT_WORKSPACE_ID.to_string())
    .execute(conn.as_mut())
    .await
    .unwrap();

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters::default(),
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert_eq!(
        items[0].labels,
        vec!["alpha".to_string(), "zeta".to_string()]
    );
    assert!(items[0].has_conflict);
    assert_eq!(items[0].blocks[0].display_ref, "APP-0000000000000302");
    assert_eq!(items[0].blocks[0].title, "resolved");
    assert_eq!(items[1].depends_on[0].display_ref, "APP-0000000000000301");
    assert_eq!(items[1].depends_on[0].title, "labeled");
    assert!(!items[1].has_conflict);
    assert!(items[2].labels.is_empty());
}

#[tokio::test]
async fn list_items_include_description_and_note_metadata() {
    let (_temp, mut conn) = test_conn().await;
    let workspace_id = crate::workspaces::default_workspace_id();
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "0000000000000501",
        "documented",
        "todo",
        "none",
        "001",
    )
    .await;
    sqlx::query("UPDATE tasks SET description = 'details' WHERE workspace_id = ? AND id = ?")
        .bind(&workspace_id)
        .bind("0000000000000501")
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
         VALUES (?, 'note-0501-a', '0000000000000501', 'older', '001', 'change-0501-a'),
                (?, 'note-0501-b', '0000000000000501', 'newer', '002', 'change-0501-b')",
    )
    .bind(&workspace_id)
    .bind(&workspace_id)
    .execute(conn.as_mut())
    .await
    .unwrap();

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters::default(),
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert_eq!(items[0].task.description, "details");
    assert_eq!(
        items[0]
            .notes
            .iter()
            .map(|note| note.body.as_str())
            .collect::<Vec<_>>(),
        ["newer", "older"]
    );
}

#[tokio::test]
async fn ranked_list_applies_limit_after_queue_ordering() {
    let (_temp, mut conn) = test_conn().await;
    let workspace_id = crate::workspaces::default_workspace_id();
    seed_default_project(&mut conn).await;

    for (id, title, status, priority, created_at) in [
        ("1000000000000001", "old inbox", "inbox", "none", "001"),
        ("1000000000000002", "urgent todo", "todo", "urgent", "002"),
        ("1000000000000003", "active", "active", "none", "003"),
        ("1000000000000004", "new inbox", "inbox", "none", "004"),
        ("1000000000000005", "high todo", "todo", "high", "005"),
        ("1000000000000006", "medium todo", "todo", "medium", "006"),
    ] {
        insert_test_task(&mut conn, id, title, status, priority, created_at).await;
    }

    let all_items = list_task_items_in_workspace(
        &mut conn,
        &workspace_id,
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    let expected = listed_titles(&all_items)[..3].to_vec();

    let bounded = list_task_summary_items_in_workspace(
        &mut conn,
        &workspace_id,
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
        Some(3),
    )
    .await
    .unwrap();

    assert_eq!(listed_titles(&bounded), expected);
}

#[tokio::test]
async fn bounded_list_hydrates_only_selected_summary_rows() {
    let (_temp, mut conn) = test_conn().await;
    let workspace_id = crate::workspaces::default_workspace_id();
    seed_default_project(&mut conn).await;

    for index in 0..40 {
        let task_id = format!("{index:016}");
        let created_at = format!("{index:03}");
        insert_test_task(
            &mut conn,
            &task_id,
            &format!("task {index:02}"),
            "todo",
            "none",
            &created_at,
        )
        .await;
        sqlx::query(
            "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&workspace_id)
        .bind(format!("note-{index:011}"))
        .bind(&task_id)
        .bind(format!("note body {index}"))
        .bind(&created_at)
        .bind(format!("note-change-{index:04}"))
        .execute(conn.as_mut())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_attachments(
                 workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                 filename, width, height, created_at
             ) VALUES (?, ?, ?, ?, 1, 'image/png', ?, 1, 1, ?)",
        )
        .bind(&workspace_id)
        .bind(format!("{index:016X}"))
        .bind(&task_id)
        .bind(format!("{index:064x}"))
        .bind(format!("image-{index}.png"))
        .bind(&created_at)
        .execute(conn.as_mut())
        .await
        .unwrap();
    }

    let all_items = list_task_items_in_workspace(
        &mut conn,
        &workspace_id,
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    let expected_ids = all_items
        .iter()
        .take(5)
        .map(|item| item.task.id.clone())
        .collect::<Vec<_>>();
    assert!(all_items.iter().all(|item| item.notes.len() == 1));
    assert!(all_items.iter().all(|item| item.attachments.len() == 1));

    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status = 'invalid' WHERE id = '0000000000000005'")
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query("ALTER TABLE notes RENAME TO detail_notes")
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query("ALTER TABLE task_attachments RENAME TO detail_task_attachments")
        .execute(conn.as_mut())
        .await
        .unwrap();

    let bounded = list_task_summary_items_in_workspace(
        &mut conn,
        &workspace_id,
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
        Some(5),
    )
    .await
    .unwrap();

    assert_eq!(
        bounded
            .iter()
            .map(|item| item.task.id.clone())
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert!(bounded.iter().all(|item| item.notes.is_empty()));
    assert!(bounded.iter().all(|item| item.attachments.is_empty()));
}

#[tokio::test]
async fn list_items_preserve_display_refs_with_hidden_collisions() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    insert_test_task(
        &mut conn,
        "ABCD000000000001",
        "visible",
        "todo",
        "none",
        "001",
    )
    .await;
    insert_test_task(&mut conn, "ABCD999999999999", "done", "done", "none", "002").await;

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].display_ref, "APP-ABCD0");
}

#[tokio::test]
async fn search_filter_matches_titles_and_descriptions() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    for (id, title, description, created_at) in [
        (
            "0000000000000501",
            "Title match needle",
            "plain body",
            "001",
        ),
        (
            "0000000000000502",
            "Body only",
            "body contains needle",
            "002",
        ),
        ("0000000000000503", "Unrelated", "plain body", "003"),
    ] {
        sqlx::query(
            "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
             VALUES (?, ?, ?, '0000000000000001', 'todo', 'none', ?, ?, ?)",
        )
        .bind(id)
        .bind(title)
        .bind(description)
        .bind(created_at)
        .bind(created_at)
        .bind(created_at)
        .execute(conn.as_mut())
        .await
        .unwrap();
    }

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            search: Some("needle".to_string()),
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert_eq!(listed_titles(&items), ["Title match needle", "Body only"]);
}

#[tokio::test]
async fn queue_sort_ranks_conflicted_tasks_ahead_of_clean_peers() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    insert_test_task(
        &mut conn,
        "0000000000000401",
        "clean",
        "todo",
        "none",
        "001",
    )
    .await;
    insert_test_task(
        &mut conn,
        "0000000000000402",
        "conflicted",
        "todo",
        "none",
        "002",
    )
    .await;
    insert_test_conflict(&mut conn, "0000000000000402", false).await;

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters::default(),
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert_eq!(listed_titles(&items), ["conflicted", "clean"]);
}

#[tokio::test]
async fn explicit_workspace_read_apis_scope_results() {
    let (_temp, mut conn) = test_conn().await;
    let alpha_id = crate::workspaces::default_workspace_id();
    let beta = crate::workspaces::create_workspace(&mut conn, "Beta")
        .await
        .unwrap();
    seed_workspace_project(&mut conn, &alpha_id, "app", "Alpha", "ALP").await;
    seed_workspace_project(&mut conn, &beta.id, "app", "Beta", "BET").await;
    seed_workspace_label(&mut conn, &alpha_id, "shared").await;
    seed_workspace_label(&mut conn, &beta.id, "shared").await;
    seed_workspace_task(
        &mut conn,
        &alpha_id,
        "A1PHA00000000001",
        "alpha task",
        "app",
        "todo",
        "high",
        "001",
    )
    .await;
    seed_workspace_task(
        &mut conn,
        &beta.id,
        "BETA000000000001",
        "beta task",
        "app",
        "done",
        "low",
        "002",
    )
    .await;
    seed_workspace_task_label(&mut conn, &alpha_id, "A1PHA00000000001", "shared").await;
    seed_workspace_task_label(&mut conn, &beta.id, "BETA000000000001", "shared").await;
    seed_workspace_conflict(&mut conn, &alpha_id, "A1PHA00000000001").await;

    let alpha_tasks = list_task_items_in_workspace(
        &mut conn,
        &alpha_id,
        TaskFilters {
            project: Some("app".to_string()),
            label: Some("shared".to_string()),
            conflicts_only: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    assert_eq!(listed_titles(&alpha_tasks), ["alpha task"]);
    assert_eq!(alpha_tasks[0].labels, vec!["shared".to_string()]);
    assert!(alpha_tasks[0].has_conflict);

    let beta_tasks = list_task_items_in_workspace(
        &mut conn,
        &beta.id,
        TaskFilters::default(),
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    assert_eq!(listed_titles(&beta_tasks), ["beta task"]);

    let alpha_projects = list_project_items_in_workspace(&mut conn, &alpha_id)
        .await
        .unwrap();
    assert_eq!(alpha_projects.len(), 1);
    assert_eq!(alpha_projects[0].key, "app");
    assert_eq!(alpha_projects[0].open_count, 1);

    let beta_projects = list_project_items_in_workspace(&mut conn, &beta.id)
        .await
        .unwrap();
    assert_eq!(beta_projects.len(), 1);
    assert_eq!(beta_projects[0].key, "app");
    assert_eq!(beta_projects[0].open_count, 0);

    let alpha_counts = sidebar_counts_for_scope_in_workspace(&mut conn, &alpha_id, None)
        .await
        .unwrap();
    assert_eq!(alpha_counts.open, 1);
    assert_eq!(alpha_counts.todo, 1);
    assert_eq!(alpha_counts.conflicts, 1);
    assert_eq!(alpha_counts.done, 0);

    let beta_counts = sidebar_counts_for_scope_in_workspace(&mut conn, &beta.id, None)
        .await
        .unwrap();
    assert_eq!(beta_counts.open, 0);
    assert_eq!(beta_counts.done, 1);
    assert_eq!(beta_counts.conflicts, 0);
}

#[tokio::test]
async fn project_filters_use_project_id_after_key_change() {
    let (_temp, mut conn) = test_conn().await;
    let outcome = crate::projects::create_project_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        "App",
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
         VALUES (?, 'ABCDEF0000000000', 'kept', '', ?, 'todo', 'none', 't', 't', 't')",
    )
    .bind(crate::workspaces::default_workspace_id())
    .bind(&outcome.project.id)
    .execute(conn.as_mut())
    .await
    .unwrap();
    sqlx::query("UPDATE projects SET key = 'renamed-app' WHERE id = ?")
        .bind(&outcome.project.id)
        .execute(conn.as_mut())
        .await
        .unwrap();

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            project: Some("renamed-app".to_string()),
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await
    .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].task.project_key, "renamed-app");
}

#[tokio::test]
async fn epics_filter_lists_explicit_epics_with_children() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    let workspace_id = crate::workspaces::default_workspace_id();

    // Parent task - open
    insert_test_task(
        &mut conn,
        "EP1C000000000001",
        "epic parent",
        "todo",
        "high",
        "001",
    )
    .await;
    // Child task - open
    insert_test_task(
        &mut conn,
        "EP1C000000000002",
        "open child",
        "active",
        "none",
        "002",
    )
    .await;
    // Another parent - done (should NOT match the filter)
    insert_test_task(
        &mut conn,
        "EP1C000000000003",
        "done parent",
        "done",
        "none",
        "003",
    )
    .await;
    // A parent with a done child (should NOT match the filter)
    insert_test_task(
        &mut conn,
        "EP1C000000000004",
        "parent with done child",
        "todo",
        "none",
        "004",
    )
    .await;
    insert_test_task(
        &mut conn,
        "EP1C000000000005",
        "done child",
        "done",
        "none",
        "005",
    )
    .await;
    // An open task with no dependents (should NOT match)
    insert_test_task(
        &mut conn,
        "EP1C000000000006",
        "lonely task",
        "todo",
        "none",
        "006",
    )
    .await;

    sqlx::query(
        "UPDATE tasks SET is_epic = 1 WHERE id IN ('EP1C000000000001', 'EP1C000000000004')",
    )
    .execute(conn.as_mut())
    .await
    .unwrap();

    // epic parent -> open child
    sqlx::query(
        "INSERT INTO task_epic_links(workspace_id, child_task_id, epic_task_id, created_at)
         VALUES (?, 'EP1C000000000002', 'EP1C000000000001', '002')",
    )
    .bind(&workspace_id)
    .execute(conn.as_mut())
    .await
    .unwrap();

    // done parent -> (no-op, parent is done)
    sqlx::query(
        "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, 'EP1C000000000001', 'EP1C000000000003', '003')",
    )
    .bind(&workspace_id)
    .execute(conn.as_mut())
    .await
    .unwrap();

    // parent with done child -> done child
    sqlx::query(
        "INSERT INTO task_epic_links(workspace_id, child_task_id, epic_task_id, created_at)
         VALUES (?, 'EP1C000000000005', 'EP1C000000000004', '005')",
    )
    .bind(&workspace_id)
    .execute(conn.as_mut())
    .await
    .unwrap();

    let items = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        TaskFilters {
            epics_only: true,
            hide_done: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    assert_eq!(
        listed_titles(&items),
        ["epic parent", "parent with done child"]
    );
    assert_eq!(items[0].epic_children.len(), 1);
    assert_eq!(
        items[0].epic_children[0].task_id.as_str(),
        "EP1C000000000002"
    );
    assert_eq!(
        items[0].epic_children[0].display_ref,
        "APP-EP1C000000000002"
    );
    assert!(items[0].epic_children[0].unresolved);
}

#[tokio::test]
async fn epic_rollups_preserve_child_outcomes_and_attention_signals() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    let workspace_id = crate::workspaces::default_workspace_id();
    for (id, title, status, updated_at) in [
        ("R0AA000000000001", "mixed epic", "todo", "001"),
        ("R0AA000000000002", "empty epic", "todo", "002"),
        ("R0AA000000000003", "late ready", "active", "003"),
        ("R0AA000000000004", "blocked child", "todo", "004"),
        ("R0AA000000000005", "deferred child", "backlog", "005"),
        ("R0AA000000000006", "done child", "done", "999"),
        ("R0AA000000000007", "canceled child", "canceled", "007"),
    ] {
        insert_test_task(&mut conn, id, title, status, "none", updated_at).await;
    }
    sqlx::query(
        "UPDATE tasks SET is_epic = 1
         WHERE id IN ('R0AA000000000001', 'R0AA000000000002')",
    )
    .execute(conn.as_mut())
    .await
    .unwrap();
    sqlx::query("UPDATE tasks SET due_on = '2000-01-01' WHERE id = 'R0AA000000000003'")
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE tasks SET available_at = '2999-01-01T00:00:00Z'
         WHERE id = 'R0AA000000000005'",
    )
    .execute(conn.as_mut())
    .await
    .unwrap();
    for child_id in [
        "R0AA000000000003",
        "R0AA000000000004",
        "R0AA000000000005",
        "R0AA000000000006",
        "R0AA000000000007",
    ] {
        sqlx::query(
            "INSERT INTO task_epic_links(workspace_id, child_task_id, epic_task_id, created_at)
             VALUES (?, ?, 'R0AA000000000001', '010')",
        )
        .bind(&workspace_id)
        .bind(child_id)
        .execute(conn.as_mut())
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, 'R0AA000000000004', 'R0AA000000000003', '011')",
    )
    .bind(&workspace_id)
    .execute(conn.as_mut())
    .await
    .unwrap();

    let items = list_task_items_in_workspace(
        &mut conn,
        &workspace_id,
        TaskFilters {
            epics_only: true,
            ..TaskFilters::default()
        },
        TaskQueryMode::Flat,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();

    let mixed = items
        .iter()
        .find(|item| item.task.title == "mixed epic")
        .unwrap()
        .epic_rollup
        .as_ref()
        .unwrap();
    assert_eq!(mixed.total, 5);
    assert_eq!(mixed.open, 3);
    assert_eq!(mixed.done, 1);
    assert_eq!(mixed.canceled, 1);
    assert_eq!(mixed.blocked, 1);
    assert_eq!(mixed.overdue, 1);
    assert_eq!(mixed.ready, 1);
    assert_eq!(mixed.latest_activity_at, "999");

    let empty = items
        .iter()
        .find(|item| item.task.title == "empty epic")
        .unwrap()
        .epic_rollup
        .as_ref()
        .unwrap();
    assert_eq!(empty.total, 0);
    assert_eq!(empty.open, 0);
    assert_eq!(empty.done, 0);
    assert_eq!(empty.canceled, 0);
    assert_eq!(empty.latest_activity_at, "002");
}

#[tokio::test]
async fn availability_transition_updates_queries_queue_band_and_counts() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    insert_test_task(
        &mut conn,
        "AVA1100000000001",
        "scheduled task",
        "inbox",
        "none",
        "2026-01-01T00:00:00Z",
    )
    .await;
    sqlx::query("UPDATE tasks SET available_at = ? WHERE id = 'AVA1100000000001'")
        .bind("2999-03-08T05:00:00Z")
        .execute(conn.as_mut())
        .await
        .unwrap();

    let available_filters = TaskFilters {
        hide_done: true,
        availability: TaskAvailabilityFilter::Available,
        ..TaskFilters::default()
    };
    let upcoming_filters = TaskFilters {
        availability: TaskAvailabilityFilter::Upcoming,
        ..TaskFilters::default()
    };
    let available = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        available_filters.clone(),
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    let upcoming = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        upcoming_filters.clone(),
        TaskQueryMode::Flat,
        TaskSort::AvailableAt,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    let counts = sidebar_counts_for_scope_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        None,
    )
    .await
    .unwrap();
    let projects =
        list_project_items_in_workspace(&mut conn, &crate::workspaces::default_workspace_id())
            .await
            .unwrap();

    assert!(available.is_empty());
    assert_eq!(listed_titles(&upcoming), ["scheduled task"]);
    assert_eq!(counts.open, 0);
    assert_eq!(counts.inbox, 0);
    assert_eq!(counts.upcoming, 1);
    assert_eq!(projects[0].open_count, 0);
    assert_eq!(projects[0].inbox_count, 0);

    sqlx::query("UPDATE tasks SET available_at = ? WHERE id = 'AVA1100000000001'")
        .bind("2026-03-08T05:00:00Z")
        .execute(conn.as_mut())
        .await
        .unwrap();

    let available = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        available_filters,
        TaskQueryMode::RankedQueue,
        TaskSort::Created,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    let upcoming = list_task_items_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        upcoming_filters,
        TaskQueryMode::Flat,
        TaskSort::AvailableAt,
        SortDirection::Asc,
    )
    .await
    .unwrap();
    let counts = sidebar_counts_for_scope_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        None,
    )
    .await
    .unwrap();
    let projects =
        list_project_items_in_workspace(&mut conn, &crate::workspaces::default_workspace_id())
            .await
            .unwrap();

    assert_eq!(listed_titles(&available), ["scheduled task"]);
    assert_eq!(available[0].queue.band, crate::queue::QueueBand::Available);
    assert!(upcoming.is_empty());
    assert_eq!(counts.open, 1);
    assert_eq!(counts.inbox, 1);
    assert_eq!(counts.upcoming, 0);
    assert_eq!(projects[0].open_count, 1);
    assert_eq!(projects[0].inbox_count, 1);
}

#[tokio::test]
async fn sidebar_counts_include_epics_count() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;

    insert_test_task(&mut conn, "EPCS00000000001", "epic", "todo", "none", "001").await;
    insert_test_task(
        &mut conn,
        "EPCS00000000002",
        "child",
        "active",
        "none",
        "002",
    )
    .await;
    insert_test_task(
        &mut conn,
        "EPCS00000000003",
        "not epic",
        "todo",
        "none",
        "003",
    )
    .await;

    insert_test_task(
        &mut conn,
        "EPCS00000000004",
        "closed epic",
        "done",
        "none",
        "004",
    )
    .await;

    sqlx::query("UPDATE tasks SET is_epic = 1 WHERE id IN ('EPCS00000000001', 'EPCS00000000004')")
        .execute(conn.as_mut())
        .await
        .unwrap();

    let counts = sidebar_counts_for_scope_in_workspace(
        &mut conn,
        &crate::workspaces::default_workspace_id(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(counts.open, 3);
    assert_eq!(counts.epics, 1);
}

#[tokio::test]
async fn project_scoped_sidebar_epics_count_excludes_other_projects() {
    let (_temp, mut conn) = test_conn().await;
    seed_default_project(&mut conn).await;
    let workspace_id = crate::workspaces::default_workspace_id();
    seed_workspace_project(&mut conn, &workspace_id, "mobile", "Mobile", "MOB").await;

    insert_test_task(
        &mut conn,
        "EPCS00000000011",
        "ordinary app task",
        "todo",
        "none",
        "001",
    )
    .await;
    seed_workspace_task(
        &mut conn,
        &workspace_id,
        "EPCS00000000012",
        "mobile epic",
        "mobile",
        "todo",
        "none",
        "002",
    )
    .await;
    sqlx::query("UPDATE tasks SET is_epic = 1 WHERE id = 'EPCS00000000012'")
        .execute(conn.as_mut())
        .await
        .unwrap();

    let counts = sidebar_counts_for_scope_in_workspace(&mut conn, &workspace_id, Some("app"))
        .await
        .unwrap();

    assert_eq!(counts.open, 1);
    assert_eq!(counts.epics, 0);
}

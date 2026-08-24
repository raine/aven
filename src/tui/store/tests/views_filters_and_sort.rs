use super::*;

#[tokio::test]
async fn availability_transition_refreshes_tasks_sidebar_and_project_counts() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (message, selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "Scheduled store task".to_string(),
                available_at: Some("2999-03-08T05:00:00Z".to_string()),
                due_on: None,
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();

    assert!(selected.is_none());
    assert!(message.contains("hidden by current filters"));
    assert!(store.tasks.is_empty());
    assert_eq!(store.counts.open, 0);
    assert_eq!(store.counts.inbox, 0);
    assert_eq!(store.counts.upcoming, 1);
    let project = store
        .projects
        .iter()
        .find(|project| project.key == "aven")
        .unwrap();
    assert_eq!(project.open_count, 0);
    assert_eq!(project.inbox_count, 0);

    store.show_view(TaskQuery::Upcoming).await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Scheduled store task");
    let task_id = store.tasks[0].task.id.clone();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE tasks SET available_at = ? WHERE id = ?")
        .bind("2026-03-08T05:00:00Z")
        .bind(&task_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    store.show_view(TaskQuery::Queue).await.unwrap();

    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.id, task_id);
    assert_eq!(
        store.tasks[0].queue.band,
        crate::queue::QueueBand::Available
    );
    assert_eq!(store.counts.open, 1);
    assert_eq!(store.counts.inbox, 1);
    assert_eq!(store.counts.upcoming, 0);
    let project = store
        .projects
        .iter()
        .find(|project| project.key == "aven")
        .unwrap();
    assert_eq!(project.open_count, 1);
    assert_eq!(project.inbox_count, 1);
}

#[tokio::test]
async fn sidebar_selection_prefers_project_scope_when_scoped() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    store
        .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();

    let selected = store.sidebar_selection().unwrap();

    assert_eq!(
        store.sidebar_entries[selected].target,
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
            "mobile-app".to_string()
        )))
    );
}

#[tokio::test]
async fn clear_filters_preserves_view_scope_and_order() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    store
        .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();
    store.show_view(TaskQuery::Todo).await.unwrap();
    store.view_state.order = TaskOrder::Priority;
    store.view_state.direction = SortDirection::Desc;
    store.view_state.filter_modifiers.label = Some("backend".to_string());
    store.view_state.projection_origin =
        super::super::TaskProjectionOrigin::ExactTasks(vec![crate::test_support::task_id(
            "task-1",
        )]);

    store.clear_filters().await.unwrap();

    assert_eq!(
        store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(store.view_state.query, TaskQuery::Todo);
    assert_eq!(store.view_state.order, TaskOrder::Priority);
    assert_eq!(store.view_state.direction, SortDirection::Desc);
    assert!(store.view_state.filter_modifiers.label.is_none());
    assert_eq!(
        store.view_state.projection_origin,
        super::super::TaskProjectionOrigin::NamedView
    );
}

#[tokio::test]
async fn show_conflicts_view_sets_conflicts_view() {
    let mut store = test_store().await;

    store.show_view(TaskQuery::Conflicts).await.unwrap();

    assert_eq!(store.view_state.query, TaskQuery::Conflicts);
    assert!(store.view_state.filters().conflicts_only);
}

#[tokio::test]
async fn queue_view_hides_done_tasks() {
    let mut store = test_store().await;
    let (_, selected) = store
        .create_task(task_draft("Finished"), None)
        .await
        .unwrap();
    store.update_status(selected, "done").await.unwrap();

    store.show_view(TaskQuery::Queue).await.unwrap();

    assert!(
        store
            .tasks
            .iter()
            .all(|item| item.task.status != TaskStatus::Done)
    );
    assert_eq!(store.counts.done, 1);
    assert!(store.sidebar_entries.iter().any(|entry| {
        entry.target == Some(SidebarEntryTarget::View(TaskQuery::Done)) && entry.count == 1
    }));
}

#[tokio::test]
async fn project_scope_hides_done_and_canceled_tasks_in_open_view() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    for (title, status) in [
        ("Open task", "todo"),
        ("Finished", "done"),
        ("Canceled", "canceled"),
    ] {
        let (_, selected) = store
            .create_task(
                TaskDraft {
                    metadata: Vec::new(),
                    title: title.to_string(),
                    project: Some("mobile-app".to_string()),
                    ..task_draft("")
                },
                None,
            )
            .await
            .unwrap();
        let selected = selected.unwrap();
        store.update_status(Some(selected), status).await.unwrap();
    }

    store
        .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();
    store.show_view(TaskQuery::Open).await.unwrap();

    let filters = store.view_state.filters();
    assert_eq!(filters.project.as_deref(), Some("mobile-app"));
    assert!(filters.hide_done);
    assert_eq!(
        store
            .tasks
            .iter()
            .map(|item| item.task.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Open task"]
    );
}

#[tokio::test]
async fn done_view_shows_done_tasks() {
    let mut store = test_store().await;
    let (_, selected) = store
        .create_task(task_draft("Finished"), None)
        .await
        .unwrap();
    let selected = selected.unwrap();
    store.update_status(Some(selected), "done").await.unwrap();

    store.show_view(TaskQuery::Done).await.unwrap();

    assert_eq!(
        store.view_state.filters().statuses,
        vec!["done", "canceled"]
    );
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Finished");
}

async fn create_search_task(store: &mut TuiStore, title: &str) -> TaskId {
    let (_, selected) = store.create_task(task_draft(title), None).await.unwrap();
    store.tasks[selected.unwrap()].task.id.clone()
}

#[tokio::test]
async fn search_view_preview_hides_deleted_ordinary_text_results() {
    let mut store = test_store().await;
    let live_id = create_search_task(&mut store, "Live needle").await;
    let deleted_id = create_search_task(&mut store, "Deleted needle").await;
    let deleted_index = store
        .tasks
        .iter()
        .position(|item| item.task.id == deleted_id)
        .unwrap();
    store
        .update_deleted(Some(deleted_index), true)
        .await
        .unwrap();

    let results = store.search_preview("needle", 10).await.unwrap();

    let ids = results
        .items
        .iter()
        .map(|result| result.task_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![live_id.as_str()]);
    assert!(!ids.contains(&deleted_id.as_str()));
    assert_eq!(results.total_matches, 1);
}

#[tokio::test]
async fn search_view_submitted_search_hides_deleted_ordinary_text_results() {
    let mut store = test_store().await;
    let live_id = create_search_task(&mut store, "Live needle").await;
    let deleted_id = create_search_task(&mut store, "Deleted needle").await;
    let deleted_index = store
        .tasks
        .iter()
        .position(|item| item.task.id == deleted_id)
        .unwrap();
    store
        .update_deleted(Some(deleted_index), true)
        .await
        .unwrap();

    store.accept_search("needle").await.unwrap();

    let ids = store
        .tasks
        .iter()
        .map(|item| item.task.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![live_id.as_str()]);
    assert!(!ids.contains(&deleted_id.as_str()));
    assert_eq!(store.view_state.query, TaskQuery::Search);
}

#[tokio::test]
async fn search_view_without_query_has_an_empty_prompt_projection() {
    let mut store = test_store().await;
    create_search_task(&mut store, "Existing task").await;

    store.show_view(TaskQuery::Search).await.unwrap();

    assert!(store.tasks.is_empty());
    assert_eq!(
        store.view_state.projection_origin,
        super::super::TaskProjectionOrigin::SearchPrompt
    );
}

#[tokio::test]
async fn search_view_keeps_zero_result_restriction() {
    let mut store = test_store().await;
    create_search_task(&mut store, "Visible task").await;

    store.accept_search("missing phrase").await.unwrap();

    assert!(store.tasks.is_empty());
    assert_eq!(store.view_state.query, TaskQuery::Search);
    assert!(matches!(
        &store.view_state.projection_origin,
        super::super::TaskProjectionOrigin::Search { query, task_ids }
            if query == "missing phrase" && task_ids.is_empty()
    ));
}

#[tokio::test]
async fn search_view_preview_returns_rendered_fields_without_full_hydration() {
    let mut store = test_store().await;
    let mut draft = task_draft("Preview needle");
    draft.is_epic = true;
    let (_, selected) = store.create_task(draft, None).await.unwrap();
    let task_id = store.tasks[selected.unwrap()].task.id.clone();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    store.set_exact_priority(Some(index), "high").await.unwrap();
    store.create_label("fast".to_string()).await.unwrap();
    store
        .update_labels(Some(index), vec!["fast".to_string()])
        .await
        .unwrap();
    store
        .add_note_to_task(&task_id, "needle note body".to_string())
        .await
        .unwrap();

    let results = store.search_preview("Preview", 10).await.unwrap();

    assert_eq!(results.total_matches, 1);
    let result = &results.items[0];
    assert_eq!(result.task_id, task_id);
    assert_eq!(result.title, "Preview needle");
    assert_eq!(result.priority, "high");
    assert_eq!(result.labels, vec!["fast"]);
    assert_eq!(
        result.matched_field,
        crate::query::SearchMatchedField::Title
    );
    assert!(!result.created_at.is_empty());
    assert!(!result.deleted);
    assert!(result.is_epic);
}

#[tokio::test]
async fn search_view_submitted_search_keeps_full_result_hydration() {
    let mut store = test_store().await;
    let blocker_id = create_search_task(&mut store, "Blocker task").await;
    let task_id = create_search_task(&mut store, "Hydrated needle").await;
    let dependent_id = create_search_task(&mut store, "Dependent task").await;

    let blocker_display_ref = store
        .tasks
        .iter()
        .find(|item| item.task.id == blocker_id)
        .map(|item| item.display_ref.clone())
        .unwrap();
    let dependent_display_ref = store
        .tasks
        .iter()
        .find(|item| item.task.id == dependent_id)
        .map(|item| item.display_ref.clone())
        .unwrap();

    let task_index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    store
        .create_label("needs-review".to_string())
        .await
        .unwrap();
    store
        .update_labels(Some(task_index), vec!["needs-review".to_string()])
        .await
        .unwrap();
    store
        .add_note_to_task(&task_id, "hydrated note".to_string())
        .await
        .unwrap();

    let pool = store.database.clone();
    seed_title_conflict_database(&pool, &task_id).await;
    store.refresh(Some(&task_id)).await.unwrap();

    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    crate::operations::add_task_dependency(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &task_id,
        &blocker_id,
    )
    .await
    .unwrap();
    drop(conn);

    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    crate::operations::add_task_dependency(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &dependent_id,
        &task_id,
    )
    .await
    .unwrap();
    drop(conn);

    store.refresh(Some(&task_id)).await.unwrap();
    store.accept_search("Hydrated").await.unwrap();

    let item = store
        .tasks
        .iter()
        .find(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(item.notes.len(), 1);
    assert_eq!(item.notes[0].body, "hydrated note");
    assert!(item.has_conflict);
    assert_eq!(item.unresolved_blocker_count, 1);
    assert_eq!(item.dependent_count, 1);
    assert_eq!(item.depends_on.len(), 1);
    assert_eq!(item.blocks.len(), 1);
    assert_eq!(item.depends_on[0].display_ref, blocker_display_ref);
    assert_eq!(item.blocks[0].display_ref, dependent_display_ref);
}

#[tokio::test]
async fn search_view_finds_done_tasks_from_queue() {
    let mut store = test_store().await;
    let (_, selected) = store
        .create_task(task_draft("Finished spotlight needle"), None)
        .await
        .unwrap();
    store.update_status(selected, "done").await.unwrap();
    store.show_view(TaskQuery::Queue).await.unwrap();
    assert!(store.tasks.is_empty());

    store.accept_search("spotlight needle").await.unwrap();

    assert_eq!(store.view_state.scope, TaskScope::Workspace);
    assert_eq!(store.view_state.query, TaskQuery::Search);
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Finished spotlight needle");
}

#[tokio::test]
async fn closed_filter_reuses_visibility_cycle_for_tasks() {
    let mut store = test_store().await;
    store
        .create_task(task_draft("Open task"), None)
        .await
        .unwrap();
    let (_, selected) = store
        .create_task(task_draft("Finished task"), None)
        .await
        .unwrap();
    store.update_status(selected, "done").await.unwrap();
    store.show_view(TaskQuery::Queue).await.unwrap();

    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Open task");

    store.toggle_closed_filter().await.unwrap();
    assert_eq!(store.tasks.len(), 2);

    store.toggle_closed_filter().await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Finished task");

    store.toggle_closed_filter().await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Open task");
}

#[tokio::test]
async fn toggle_deleted_filter_switches_include_deleted() {
    let mut store = test_store().await;

    store.toggle_deleted_filter().await.unwrap();
    assert!(store.view_state.filter_modifiers.include_deleted);
    assert!(!store.view_state.filter_modifiers.deleted_only);

    store.toggle_deleted_filter().await.unwrap();
    assert!(store.view_state.filter_modifiers.include_deleted);
    assert!(store.view_state.filter_modifiers.deleted_only);

    store.toggle_deleted_filter().await.unwrap();
    assert!(!store.view_state.filter_modifiers.include_deleted);
    assert!(!store.view_state.filter_modifiers.deleted_only);
}

#[tokio::test]
async fn deleted_filter_cycle_preserves_project_scope() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    create_task_in_project(&mut store, "Live project task", "mobile-app").await;
    let selected = create_task_in_project(&mut store, "Deleted project task", "mobile-app").await;
    store.update_deleted(Some(selected), true).await.unwrap();
    store
        .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Live project task");

    store.toggle_deleted_filter().await.unwrap();

    assert_eq!(
        store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert!(store.view_state.filter_modifiers.include_deleted);
    assert!(!store.view_state.filter_modifiers.deleted_only);
    assert_eq!(store.tasks.len(), 2);

    store.toggle_deleted_filter().await.unwrap();

    assert!(store.view_state.filter_modifiers.include_deleted);
    assert!(store.view_state.filter_modifiers.deleted_only);
    assert_eq!(store.tasks.len(), 1);
    assert!(store.tasks[0].task.deleted);
}

#[tokio::test]
async fn ordering_from_queue_switches_to_open() {
    let mut store = test_store().await;

    store.set_order(TaskOrder::Priority).await.unwrap();
    assert_eq!(store.view_state.query, TaskQuery::Open);
    assert_eq!(store.view_state.order, TaskOrder::Priority);
    assert_eq!(store.view_state.direction, SortDirection::Asc);

    store.reverse_sort().await.unwrap();
    assert_eq!(store.view_state.query, TaskQuery::Open);
    assert_eq!(store.view_state.direction, SortDirection::Desc);
}

#[tokio::test]
async fn upcoming_keeps_availability_as_effective_order() {
    let mut store = test_store().await;
    store.show_view(TaskQuery::Upcoming).await.unwrap();
    store.set_order(TaskOrder::DueOn).await.unwrap();
    store.reverse_sort().await.unwrap();

    assert_eq!(store.view_state.query, TaskQuery::Upcoming);
    assert_eq!(store.view_state.sort(), crate::query::TaskSort::AvailableAt);
    assert_eq!(store.view_state.sort_direction(), SortDirection::Asc);
    assert_eq!(store.sort_label(), "available");
    assert_eq!(store.sort_direction_label(), "asc");
}

#[tokio::test]
async fn timestamp_orders_default_to_descending_and_can_toggle() {
    let mut store = test_store().await;

    store.set_order(TaskOrder::Updated).await.unwrap();
    assert_eq!(store.view_state.query, TaskQuery::Open);
    assert_eq!(store.view_state.order, TaskOrder::Updated);
    assert_eq!(store.view_state.direction, SortDirection::Desc);

    store.reverse_sort().await.unwrap();
    assert_eq!(store.view_state.direction, SortDirection::Asc);

    store.set_order(TaskOrder::Created).await.unwrap();
    assert_eq!(store.view_state.order, TaskOrder::Created);
    assert_eq!(store.view_state.direction, SortDirection::Desc);

    store.reverse_sort().await.unwrap();
    assert_eq!(store.view_state.direction, SortDirection::Asc);
}

#[tokio::test]
async fn query_anchor_preserves_task_identity_across_sorting() {
    let mut store = test_store().await;
    for title in ["Zulu task", "Alpha task", "Middle task"] {
        store.create_task(task_draft(title), None).await.unwrap();
    }
    let selected_index = 1;
    let task_id = store.tasks[selected_index].task.id.clone();
    let restore = SelectionRestore::Anchor(MainRowAnchor {
        identity: MainRowIdentity::Task(task_id.clone()),
        position: MainRowPosition::Flat(selected_index),
    });

    let selected = store
        .set_order_restoring(TaskOrder::Title, &restore)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(store.tasks[selected].task.id, task_id);
    assert_ne!(selected, selected_index);
}

#[tokio::test]
async fn query_anchor_uses_clamped_flat_position_when_task_is_hidden() {
    let mut store = test_store().await;
    for title in ["First urgent", "Second urgent"] {
        store
            .create_task(
                TaskDraft {
                    priority: "urgent".to_string(),
                    ..task_draft(title)
                },
                None,
            )
            .await
            .unwrap();
    }
    store
        .create_task(task_draft("Hidden task"), None)
        .await
        .unwrap();
    let selected_index = store
        .tasks
        .iter()
        .position(|item| item.task.title == "Hidden task")
        .unwrap();
    let task_id = store.tasks[selected_index].task.id.clone();
    let restore = SelectionRestore::Anchor(MainRowAnchor {
        identity: MainRowIdentity::Task(task_id),
        position: MainRowPosition::Flat(selected_index),
    });

    let selected = store
        .filter_priority_restoring("urgent".to_string(), &restore)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(store.tasks.len(), 2);
    assert_eq!(selected, selected_index.min(1));
    assert!(store.tasks[selected].task.priority == TaskPriority::Urgent);
}

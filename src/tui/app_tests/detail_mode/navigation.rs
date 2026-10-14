use super::*;

#[tokio::test]
async fn opening_detail_rebinds_after_the_summary_task_disappears() {
    let mut app = test_app().await;
    let created = create_and_select_task(&mut app, test_task_draft("Vanishing detail")).await;
    let task_id = app.store.tasks[created].task.id.clone();
    app.store.refresh(None).await.unwrap();
    let selected = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    app.list.select_task(Some(selected));
    assert_eq!(
        app.store.tasks[selected].hydration,
        crate::query::TaskItemHydration::Summary
    );

    let database = app.store.database();
    let mut conn = aven_core::test_support::acquire(&database).await.unwrap();
    sqlx::query("DELETE FROM tasks WHERE id = ?")
        .bind(&task_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    app.execute(crate::tui::event::Action::ToggleDetail)
        .await
        .unwrap();

    assert!(app.detail.is_inactive());
    assert!(app.store.tasks.iter().all(|item| item.task.id != task_id));
    assert_eq!(
        toast_message(&app),
        Some("task is no longer available".to_string())
    );
}

#[tokio::test]
async fn go_to_blocker_reports_no_upstream_blockers() {
    for downstream in [false, true] {
        let mut app = test_app().await;
        let selected = if downstream {
            let (blocker_id, _blocked_id) = create_blocked_pair(&mut app).await;
            app.store
                .tasks
                .iter()
                .position(|item| item.task.id == blocker_id)
                .unwrap()
        } else {
            create_and_select_task(&mut app, test_task_draft("Unblocked task")).await
        };
        app.list.select_task(Some(selected));

        for code in [KeyCode::Char('g'), KeyCode::Char('B')] {
            app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
        }

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("selected task has no blockers")
        );
    }
}

#[tokio::test]
async fn go_to_blocker_opens_one_blocker_and_back_restores_filtered_source() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE tasks SET status = 'todo' WHERE id = ?")
        .bind(&blocker_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    app.store.view_state.query = TaskQuery::Inbox;
    app.store.refresh(Some(&blocked_id)).await.unwrap();
    app.list.select_task(Some(0));

    for code in [KeyCode::Char('g'), KeyCode::Char('B')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }

    assert_eq!(app.store.view_state.query, TaskQuery::Search);
    assert_eq!(app.store.tasks[0].task.id, blocker_id);
    assert!(app.detail.is_active());

    for code in [KeyCode::Char('g'), KeyCode::Char('[')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }

    assert_eq!(app.store.view_state.query, TaskQuery::Inbox);
    assert_eq!(app.store.tasks[0].task.id, blocked_id);
    assert!(app.detail.is_active());

    for code in [KeyCode::Char('g'), KeyCode::Char('B')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert_eq!(app.store.tasks[0].task.id, blocker_id);
    assert!(app.detail.is_active());

    for code in [KeyCode::Char('g'), KeyCode::Char('[')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert_eq!(app.store.tasks[0].task.id, blocked_id);
}

#[tokio::test]
async fn go_to_blocker_picker_cancels_or_opens_selected_upstream_blocker() {
    let mut app = test_app().await;
    let (first_blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    let second_blocker_index =
        create_and_select_task(&mut app, test_task_draft("Second blocker")).await;
    let second_blocker_id = app.store.tasks[second_blocker_index].task.id.clone();
    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    app.store
        .add_dependency(Some(blocked_index), &second_blocker_id)
        .await
        .unwrap();
    app.store.refresh(Some(&blocked_id)).await.unwrap();
    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    app.list.select_task(Some(blocked_index));

    app.execute(crate::tui::event::Action::GoToBlocker)
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::GoToBlocker { .. },
            items,
            ..
        })) if items.iter().map(|item| item.value.as_str()).collect::<Vec<_>>()
            == vec![first_blocker_id.as_str(), second_blocker_id.as_str()]
    ));

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.store.tasks[blocked_index].task.id, blocked_id);

    app.execute(crate::tui::event::Action::GoToBlocker)
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Down), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Search);
    assert_eq!(app.store.tasks[0].task.id, second_blocker_id);
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_sibling_navigation_rebinds_when_the_summary_task_disappears() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("Resident detail")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("Vanishing sibling")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.store.refresh(Some(&first_id)).await.unwrap();
    let first = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == first_id)
        .unwrap();
    let second = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == second_id)
        .unwrap();
    app.list.select_task(Some(first));
    app.show_detail(0);

    let database = app.store.database();
    let mut conn = aven_core::test_support::acquire(&database).await.unwrap();
    sqlx::query("DELETE FROM tasks WHERE id = ?")
        .bind(&second_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let delta = if second > first { 1 } else { -1 };
    app.select_detail_task(delta).await.unwrap();

    assert!(app.detail.is_inactive());
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.id, first_id);
    assert_eq!(app.list.selected_task(), Some(0));
}

#[tokio::test]
async fn detail_next_and_previous_task_stay_in_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    let first = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.title == "First")
        .unwrap();
    let second = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.title == "Second")
        .unwrap();
    app.list.select_task(Some(first));
    app.show_detail(7);

    app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.list.selected_task(), Some(second));
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(toast_message(&app), None);

    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.list.selected_task(), Some(first));
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn focused_relationship_routes_status_picker_to_target() {
    for section in [
        DetailSection::DependsOn,
        DetailSection::Blocks,
        DetailSection::EpicParent,
    ] {
        let mut app = test_app().await;
        let root = create_and_select_task(&mut app, test_task_draft("Detail root")).await;
        let root_id = app.store.tasks[root].task.id.clone();
        let target = create_and_select_task(
            &mut app,
            TaskDraft {
                is_epic: section == DetailSection::EpicParent,
                ..test_task_draft("Related target")
            },
        )
        .await;
        let target = app.store.tasks[target].clone();
        let root = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == root_id)
            .unwrap();
        match section {
            DetailSection::DependsOn => {
                app.store.tasks[root].depends_on = vec![detail_link(&target)]
            }
            DetailSection::Blocks => app.store.tasks[root].blocks = vec![detail_link(&target)],
            DetailSection::EpicParent => {
                app.store.tasks[root].epic_parent = Some(detail_link(&target))
            }
            _ => unreachable!(),
        }
        app.list.select_task(Some(root));
        app.show_detail(0);
        app.detail
            .state_mut()
            .unwrap()
            .set_focused_target(Some(DetailTargetId::Task {
                section,
                task_id: target.task.id.clone(),
            }));

        app.dispatch_key(key(KeyCode::Char('s')), (80, 24).into())
            .await
            .unwrap();
        assert!(
            matches!(
                &app.footer_choice,
                Some(choice) if choice.mode == FooterChoiceMode::Status
                    && choice.selection.single_id() == Some(&target.task.id)
            ),
            "section {section:?}"
        );

        if section == DetailSection::DependsOn {
            app.footer_choice = None;
            for code in [KeyCode::Char('t'), KeyCode::Char('U')] {
                app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
            }
            assert!(matches!(
                &app.overlay,
                Some(OverlayState::Confirm(ConfirmState {
                    intent: ConfirmIntent::UnlinkDependency { selection, depends_on_task_id },
                    ..
                })) if selection.single_id() == Some(&root_id) && depends_on_task_id == &target.task.id
            ));
        }
    }
}

#[tokio::test]
async fn detail_tab_focuses_epic_children_and_j_k_selects_and_opens() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, _child_ids) = create_epic_with_children(
        &mut app,
        &pool,
        "Parent epic",
        &["First child", "Second child"],
    )
    .await;
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(4);
    let child_ids = app.store.tasks[parent_index]
        .epic_children
        .iter()
        .map(|child| child.task_id.clone())
        .collect::<Vec<_>>();

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id)
            .map(crate::ids::TaskId::as_str),
        Some(child_ids[0].as_str())
    );
    assert_eq!(
        app.view()
            .detail_focus
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id)
            .map(crate::ids::TaskId::as_str),
        Some(child_ids[0].as_str())
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('j')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id)
            .map(crate::ids::TaskId::as_str),
        Some(child_ids[1].as_str())
    );

    app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id)
            .map(crate::ids::TaskId::as_str),
        Some(child_ids[0].as_str())
    );

    app.dispatch_key(key(KeyCode::Char('j')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, child_ids[1]);
    assert!(app.detail.state_mut().unwrap().focused_target.is_none());
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, parent_id);
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id),
        Some(&child_ids[1])
    );
}

#[tokio::test]
async fn missing_linked_task_keeps_current_detail_open() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Current task")).await;
    let current_id = app.store.tasks[selected].task.id.clone();
    let missing_id = crate::test_support::task_id("missing-linked-task");
    app.store.tasks[selected].depends_on = vec![crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: missing_id,
        display_ref: "APP-MISS".to_string(),
        title: "Unavailable blocker".to_string(),
        status: "todo".to_string(),
        priority: "high".to_string(),
        unresolved: true,
    }];
    app.show_detail(2);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.tasks[selected].task.id, current_id);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("linked task is unavailable")
    );
    assert!(app.detail.state_mut().unwrap().history.is_empty());
}

#[test]
fn focused_detail_policy_matches_target_section() {
    use crate::tui::event::{DetailFocusPolicy, RoutingDomain, focus_policy_compatible};

    let regular = crate::tui::app::DetailTargetId::Task {
        section: crate::tui::app::DetailSection::DependsOn,
        task_id: crate::test_support::task_id("policy-regular"),
    };
    let child = crate::tui::app::DetailTargetId::Task {
        section: crate::tui::app::DetailSection::EpicChildren,
        task_id: crate::test_support::task_id("policy-child"),
    };
    let attachment = crate::tui::app::DetailTargetId::Attachment {
        attachment_id: "policy-attachment".to_string(),
    };
    let note = crate::tui::app::DetailTargetId::Note {
        note_id: "policy-note".to_string(),
    };
    let cases = [
        (DetailFocusPolicy::Global, &regular, true),
        (DetailFocusPolicy::RelatedTask, &regular, true),
        (DetailFocusPolicy::EpicChild, &regular, false),
        (DetailFocusPolicy::EpicChild, &child, true),
        (DetailFocusPolicy::ParentTask, &regular, false),
        (DetailFocusPolicy::Global, &attachment, true),
        (DetailFocusPolicy::RelatedTask, &attachment, false),
        (DetailFocusPolicy::Global, &note, true),
        (DetailFocusPolicy::RelatedTask, &note, false),
    ];
    for (policy, target, expected) in cases {
        let section = matches!(target, crate::tui::app::DetailTargetId::Task { .. })
            .then(|| target.section());
        let domain = target.routing_domain();
        assert_eq!(
            focus_policy_compatible(policy, domain, section),
            expected,
            "policy {policy:?} target {target:?}"
        );
    }
    assert!(focus_policy_compatible(
        DetailFocusPolicy::ParentTask,
        RoutingDomain::DetailParent,
        None
    ));
}

#[tokio::test]
async fn focused_detail_missing_relationship_reports_unavailable_without_mutating_parent() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Relationship target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let linked_id = crate::test_support::task_id("focused-linked-task");
    app.store.tasks[selected].depends_on = vec![crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: linked_id.clone(),
        display_ref: "APP-LINK".to_string(),
        title: "Linked task".to_string(),
        status: "todo".to_string(),
        priority: "high".to_string(),
        unresolved: true,
    }];
    app.show_detail(0);
    app.detail.state_mut().unwrap().focused_target = Some(crate::tui::app::DetailTargetId::Task {
        section: crate::tui::app::DetailSection::DependsOn,
        task_id: linked_id,
    });

    app.dispatch_key(key(KeyCode::Char('s')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.tasks[selected].task.id, task_id);
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("linked task is unavailable")
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn linked_task_opens_outside_active_list_filter() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Filtered child"]).await;
    let child_id = child_ids[0].clone();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE tasks SET status = 'todo' WHERE id = ?")
        .bind(&child_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    app.store.view_state.query = TaskQuery::Inbox;
    app.store.refresh(Some(&parent_id)).await.unwrap();
    assert_eq!(app.store.tasks.len(), 1);
    app.list.select_task(Some(0));
    app.show_detail(3);
    app.detail
        .state_mut()
        .unwrap()
        .expanded_sections
        .insert(crate::tui::app::DetailSection::Blocks);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Search);
    assert_eq!(app.store.tasks[0].task.id, child_id);
    assert!(app.detail.state_mut().unwrap().expanded_sections.is_empty());
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.store.tasks[0].task.id, child_id);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no other tasks in source list")
    );

    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Inbox);
    assert_eq!(app.store.tasks[0].task.id, parent_id);
    assert!(
        app.detail
            .state_mut()
            .unwrap()
            .expanded_sections
            .contains(&crate::tui::app::DetailSection::Blocks)
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(crate::tui::app::DetailTargetId::task_id),
        Some(&child_id)
    );
}

#[tokio::test]
async fn in_filter_linked_task_uses_its_source_list_position_for_siblings() {
    let mut app = test_app().await;
    let source = create_and_select_task(&mut app, test_task_draft("Source task")).await;
    let source_id = app.store.tasks[source].task.id.clone();
    let linked = create_and_select_task(&mut app, test_task_draft("Linked task")).await;
    let linked_id = app.store.tasks[linked].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Following task")).await;
    let source = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == source_id)
        .unwrap();
    let linked = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == linked_id)
        .unwrap();
    let next_id = app.store.tasks[(linked + 1) % app.store.tasks.len()]
        .task
        .id
        .clone();
    app.list.select_task(Some(source));
    app.show_detail(3);

    app.open_detail_task(&linked_id, 3).await;
    app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&next_id)
    );

    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&linked_id)
    );

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&source_id)
    );
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn hidden_linked_task_navigates_previous_and_next_in_source_list() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    create_and_select_task(&mut app, test_task_draft("Before source")).await;
    let source = create_and_select_task(&mut app, test_task_draft("Source task")).await;
    let source_id = app.store.tasks[source].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("After source")).await;
    let hidden = create_and_select_task(&mut app, test_task_draft("Hidden linked task")).await;
    let hidden_id = app.store.tasks[hidden].task.id.clone();
    sqlx::query("UPDATE tasks SET status = 'todo' WHERE id = ?")
        .bind(&hidden_id)
        .execute(&pool)
        .await
        .unwrap();
    app.store.view_state.query = TaskQuery::Inbox;
    app.store.refresh(Some(&source_id)).await.unwrap();
    let source = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == source_id)
        .unwrap();
    let previous_id = app.store.tasks[(source + app.store.tasks.len() - 1) % app.store.tasks.len()]
        .task
        .id
        .clone();
    let next_id = app.store.tasks[(source + 1) % app.store.tasks.len()]
        .task
        .id
        .clone();
    app.list.select_task(Some(source));
    app.show_detail(4);

    app.open_detail_task(&hidden_id, 4).await;
    assert_eq!(app.store.tasks[0].task.status, TaskStatus::Todo);
    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Inbox);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&previous_id)
    );

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&source_id)
    );

    app.open_detail_task(&hidden_id, 4).await;
    app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Inbox);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&next_id)
    );

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&source_id)
    );
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn deleted_linked_task_navigates_in_source_list_and_returns_to_parent() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let source = create_and_select_task(&mut app, test_task_draft("Source task")).await;
    let source_id = app.store.tasks[source].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Next source task")).await;
    let deleted = create_and_select_task(&mut app, test_task_draft("Deleted linked task")).await;
    let deleted_id = app.store.tasks[deleted].task.id.clone();
    sqlx::query("UPDATE tasks SET deleted = 1 WHERE id = ?")
        .bind(&deleted_id)
        .execute(&pool)
        .await
        .unwrap();
    app.store.refresh(Some(&source_id)).await.unwrap();
    let source = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == source_id)
        .unwrap();
    let next_id = app.store.tasks[(source + 1) % app.store.tasks.len()]
        .task
        .id
        .clone();
    app.list.select_task(Some(source));
    app.show_detail(5);

    app.open_detail_task(&deleted_id, 5).await;
    assert!(app.store.tasks[0].task.deleted);
    app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&next_id)
    );

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&source_id)
    );
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn unavailable_detail_history_keeps_current_task_open() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Current linked task")).await;
    let current = app.store.tasks[selected].clone();
    let current_id = current.task.id.clone();
    app.store.show_exact_task(current);
    app.list.select_task(Some(0));
    app.show_detail(4);
    let current_view = app.store.view_state.clone();
    let missing_id = crate::test_support::task_id("missing-history-task");
    app.push_detail_navigation_state(crate::tui::detail_session::DetailSnapshot {
        task_id: missing_id.clone(),
        scroll: 2,
        focused_target: None,
        expanded_sections: Default::default(),
        view_state: crate::tui::store::TaskViewState::for_exact_task(missing_id),
    });

    assert!(!app.go_back_in_detail().await.unwrap());
    assert_eq!(app.store.view_state, current_view);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&current_id)
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_lifecycle_preserves_parent_state_and_reopens_cleanly() {
    let mut app = test_app().await;
    let parent = create_and_select_task(&mut app, test_task_draft("Selectable parent")).await;
    let parent_id = app.store.tasks[parent].task.id.clone();
    let child = create_and_select_task(&mut app, test_task_draft("Linked child")).await;
    let child_id = app.store.tasks[child].task.id.clone();
    let parent = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent));
    app.show_detail(3);
    if app.detail.is_inactive() {
        app.show_detail(0);
    }
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Task {
        section: DetailSection::Blocks,
        task_id: child_id.clone(),
    });
    app.detail
        .state_mut()
        .unwrap()
        .expanded_sections
        .insert(DetailSection::Blocks);
    let size = (80, 24).into();
    app.dispatch_mouse(left_click(0, 3), size).await.unwrap();
    app.dispatch_mouse(left_drag(10, 3), size).await.unwrap();
    app.dispatch_mouse(left_release(10, 3), size).await.unwrap();
    let parent_selection = app
        .detail
        .state_mut()
        .unwrap()
        .text_selection
        .clone()
        .unwrap();
    let parent_text = crate::tui::ui::detail_selected_text(
        &crate::tui::theme::Theme::dark(),
        app.store.selected_task(Some(parent)).unwrap(),
        &parent_selection,
    )
    .unwrap();

    app.open_detail_task(&child_id, 3).await;

    assert_eq!(app.store.tasks[0].task.id, child_id);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.detail.state_mut().unwrap().focused_target.is_none());
    assert!(app.detail.state_mut().unwrap().expanded_sections.is_empty());
    assert!(app.detail.state_mut().unwrap().text_selection.is_some());
    assert!(
        crate::tui::ui::detail_selected_text(
            &crate::tui::theme::Theme::dark(),
            &app.store.tasks[0],
            &parent_selection
        )
        .is_none()
    );
    assert!(app.detail_has_parent());

    app.dispatch_key(key(KeyCode::Char('g')), size)
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('[')), size)
        .await
        .unwrap();

    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&parent_id)
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );
    assert!(
        app.detail
            .state_mut()
            .unwrap()
            .expanded_sections
            .contains(&DetailSection::Blocks)
    );
    assert_eq!(
        crate::tui::ui::detail_selected_text(
            &crate::tui::theme::Theme::dark(),
            app.store.selected_task(app.list.selected_task()).unwrap(),
            &parent_selection,
        )
        .as_deref(),
        Some(parent_text.as_str())
    );
    assert!(!app.detail_has_parent());

    app.dispatch_key(key(KeyCode::Char('q')), size)
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_inactive());

    app.activate_or_toggle_detail().await.unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.detail.state_mut().unwrap().focused_target.is_none());
    assert!(app.detail.state_mut().unwrap().expanded_sections.is_empty());
    assert!(app.detail.state_mut().unwrap().text_selection.is_none());
    assert!(app.detail.state_mut().unwrap().history.is_empty());
}

use super::*;

#[tokio::test]
async fn epic_detail_add_child_shortcut_opens_contextual_search() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();
    let display_ref = app.store.tasks[parent_index].display_ref.clone();
    app.show_detail(3);

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected add-child search");
    };
    assert!(matches!(
        &state.intent,
        SearchIntent::AddEpicChild {
            epic_id,
            display_ref: purpose_ref,
            ..
        } if epic_id == &parent_id && purpose_ref == &display_ref
    ));
    assert!(state.results[0].create_new);
}

#[tokio::test]
async fn ordinary_task_add_child_confirms_promotion_and_links_child() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let parent_index = create_and_select_task(&mut app, test_task_draft("Ordinary parent")).await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();
    let parent_ref = app.store.tasks[parent_index].display_ref.clone();
    let child_index =
        create_and_select_task(&mut app, test_task_draft("Promotion link target")).await;
    let child_id = app.store.tasks[child_index].task.id.clone();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }

    let Some(OverlayState::Confirm(state)) = &app.overlay else {
        panic!("expected promotion confirmation");
    };
    assert!(matches!(
        &state.intent,
        ConfirmIntent::PromoteTaskForChild { epic }
            if epic.epic_id == parent_id && epic.display_ref == parent_ref
    ));
    assert!(!app.store.tasks[parent_index].task.is_epic);

    app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    type_chars(&mut app, "Promotion link target").await;
    settle_search_preview(&mut app).await;
    let Some(OverlayState::Search(state)) = &mut app.overlay else {
        panic!("expected add-child search");
    };
    state.selected = state
        .results
        .iter()
        .position(|result| result.task_id == child_id)
        .expect("existing child result");

    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();

    let parent = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == parent_id)
        .unwrap();
    assert!(parent.task.is_epic);
    assert!(
        parent
            .epic_children
            .iter()
            .any(|child| child.task_id == child_id)
    );
}

#[tokio::test]
async fn canceling_ordinary_task_promotion_keeps_task_ordinary() {
    let mut app = test_app().await;
    let parent_index = create_and_select_task(&mut app, test_task_draft("Ordinary parent")).await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.handle_normal_key(code).await.unwrap();
    }
    assert!(matches!(app.overlay, Some(OverlayState::Confirm(_))));

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == parent_id)
            .is_some_and(|item| !item.task.is_epic)
    );
}

#[tokio::test]
async fn canceling_new_epic_child_authoring_preserves_list_origin() {
    let mut app = test_app().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.handle_normal_key(code).await.unwrap();
    }
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
    assert!(matches!(
        app.authoring
            .submission_context()
            .map(|context| context.origin),
        Some(AddTaskOrigin::EpicChild { .. })
    ));

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    assert!(!app.detail.is_active());

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(!app.detail.is_active());
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&parent_id)
    );
}

#[tokio::test]
async fn submitting_new_epic_child_uses_owned_origin_and_focuses_child() {
    let mut app = test_app().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.handle_normal_key(code).await.unwrap();
    }
    type_chars(&mut app, "Owned child").await;
    settle_search_preview(&mut app).await;
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.authoring.is_idle());
    assert!(app.detail.is_active());
    let parent = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == parent_id)
        .unwrap();
    let child = parent
        .epic_children
        .iter()
        .find(|child| child.title == "Owned child")
        .unwrap();
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&child.task_id)
    );
}

#[tokio::test]
async fn submitting_new_epic_child_from_linked_detail_returns_to_epic() {
    let mut app = test_app().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();
    let parent_ref = app.store.tasks[parent_index].display_ref.clone();
    let project_key = app.store.tasks[parent_index].task.project_key.clone();
    let child_index = create_and_select_task(&mut app, test_task_draft("Existing child")).await;
    let child_id = app.store.tasks[child_index].task.id.clone();
    app.store
        .add_epic_child(
            crate::tui::store::EpicContext {
                epic_id: parent_id.clone(),
                display_ref: parent_ref,
                project_key,
            },
            child_id.clone(),
        )
        .await
        .unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(0);
    app.open_detail_task(&child_id, 0).await;

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    type_chars(&mut app, "Linked detail child").await;
    settle_search_preview(&mut app).await;
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&parent_id)
    );
    assert!(app.detail.is_active());
    assert!(matches!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            ..
        })
    ));
}

#[tokio::test]
async fn canceling_new_epic_child_authoring_returns_to_detail_origin() {
    let mut app = test_app().await;
    create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    app.show_detail(3);

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
    assert!(matches!(
        app.authoring
            .submission_context()
            .map(|context| context.origin),
        Some(AddTaskOrigin::EpicChild { .. })
    ));

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn epic_detail_add_child_search_hides_other_projects() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent needle")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();
    let parent_project = app.store.tasks[parent_index].task.project_key.clone();
    create_and_select_task(&mut app, test_task_draft("Same project needle")).await;
    create_and_select_task(
        &mut app,
        TaskDraft {
            project: Some("other".to_string()),
            ..test_task_draft("Other project needle")
        },
    )
    .await;
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(0);

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    type_chars(&mut app, "needle").await;
    settle_search_preview(&mut app).await;

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected add-child search");
    };
    assert!(
        state
            .results
            .iter()
            .filter(|result| !result.create_new)
            .all(|result| result.project_key == parent_project)
    );
    assert!(
        state
            .results
            .iter()
            .any(|result| result.title == "Same project needle")
    );
    assert!(
        state
            .results
            .iter()
            .all(|result| result.title != "Other project needle")
    );
}

#[tokio::test]
async fn epic_child_search_applies_result_limit_within_parent_project() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();
    let target_index = create_and_select_task(
        &mut app,
        test_task_draft("GitHub issue: Dangerous speed adjustment"),
    )
    .await;
    let target_id = app.store.tasks[target_index].task.id.clone();
    for index in 0..SEARCH_PREVIEW_LIMIT {
        create_and_select_task(
            &mut app,
            TaskDraft {
                title: "GitHub".to_string(),
                project: Some(format!("other-{index}")),
                ..test_task_draft("")
            },
        )
        .await;
    }
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    type_chars(&mut app, "github").await;
    settle_search_preview(&mut app).await;

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected add-child search");
    };
    assert!(
        state
            .results
            .iter()
            .any(|result| result.task_id == target_id)
    );
}

#[tokio::test]
async fn epic_child_search_tab_cycles_results() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();
    create_and_select_task(&mut app, test_task_draft("Selectable child")).await;
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('a')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    type_chars(&mut app, "Selectable child").await;
    settle_search_preview(&mut app).await;
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.selected == 0 && state.results.len() == 2
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.selected == 1
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.selected == 0
    ));

    app.handle_overlay_key(key(KeyCode::BackTab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.selected == 1
    ));
}

#[tokio::test]
async fn right_navigation_toggles_selected_epic() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    let child_id = child_ids[0].clone();
    app.store.show_view(TaskView::Epics).await.unwrap();

    for code in [KeyCode::Char('l'), KeyCode::Right] {
        let parent_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == parent_id)
            .unwrap();
        app.list.select_task(Some(parent_index));

        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();

        assert!(app.store.view_state.expanded_epic_ids.contains(&parent_id));
        assert!(app.store.tasks.iter().any(|item| item.task.id == child_id));

        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();

        assert!(app.store.view_state.collapsed_epic_ids.contains(&parent_id));
        assert!(!app.store.tasks.iter().any(|item| item.task.id == child_id));
    }
}

#[tokio::test]
async fn focused_detail_child_back_shortcut_uses_detail_navigation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, _child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();

    assert!(!app.detail.is_active());
    assert!(app.overlay.is_none());
    assert_ne!(
        toast_message(&app).as_deref(),
        Some("no previous navigation state")
    );
}

#[tokio::test]
async fn focused_detail_child_return_shortcut_targets_recent_change() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    let child_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == child_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(0);
    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    app.list.record_changed_task(child_id.clone());

    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('.')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.detail.is_active());
    assert_eq!(app.list.selected_task(), Some(child_index));
    assert!(
        app.detail
            .state()
            .is_some_and(|detail| detail.focused_target().is_none())
    );
}

#[tokio::test]
async fn stale_detail_child_focus_is_cleared_before_shortcuts() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Parent epic")).await;
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: crate::test_support::task_id("stale-child"),
        }));

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();

    assert!(
        app.detail
            .state()
            .is_some_and(|detail| detail.focused_target().is_none())
    );
    assert!(app.pending_shortcut.is_empty());
    assert!(app.overlay.is_none());
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
}

#[tokio::test]
async fn focused_detail_child_routes_actions_and_preserves_parent_detail() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );

    for code in [KeyCode::Char('y'), KeyCode::Char('t')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert_eq!(toast_message(&app).as_deref(), Some("copied task title"));
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&parent_id)
    );
    assert_eq!(app.store.tasks[0].task.id, parent_id);
    assert_eq!(app.store.tasks[0].task.status, TaskStatus::Inbox);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .epic_children
            .iter()
            .find(|child| child.task_id == child_id)
            .map(|child| child.status.as_str()),
        Some("done")
    );
    assert_eq!(
        app.store
            .load_task_item(&child_id)
            .await
            .unwrap()
            .unwrap()
            .task
            .status,
        TaskStatus::Done
    );
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );

    app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&parent_id)
    );
    assert_eq!(
        app.store
            .load_task_item(&child_id)
            .await
            .unwrap()
            .unwrap()
            .task
            .status,
        TaskStatus::Inbox
    );
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .epic_children
            .iter()
            .find(|child| child.task_id == child_id)
            .map(|child| child.status.as_str()),
        Some("inbox")
    );
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );

    app.dispatch_key(key(KeyCode::Char('?')), (80, 24).into())
        .await
        .unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::DetailHelp { .. })));
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );
}

#[tokio::test]
async fn focused_blocker_status_picker_targets_blocker() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    app.store.refresh(Some(&blocked_id)).await.unwrap();
    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    app.list.select_task(Some(blocked_index));
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::DependsOn,
            task_id: blocker_id.clone(),
        }));

    app.dispatch_key(key(KeyCode::Char('s')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.footer_choice
            .as_ref()
            .and_then(|choice| choice.selection.single_id()),
        Some(&blocker_id)
    );
    app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.tasks[0].task.id, blocked_id);
    assert_eq!(app.store.tasks[0].task.status, TaskStatus::Inbox);
    assert_eq!(
        app.store
            .load_task_item(&blocker_id)
            .await
            .unwrap()
            .unwrap()
            .task
            .status,
        TaskStatus::Done
    );
}

#[tokio::test]
async fn focused_blocked_task_unlink_requires_confirmation() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    app.store.refresh(Some(&blocker_id)).await.unwrap();
    let blocker_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocker_id)
        .unwrap();
    app.list.select_task(Some(blocker_index));
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::Blocks,
            task_id: blocked_id.clone(),
        }));

    for code in [KeyCode::Char('t'), KeyCode::Char('U')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkDependency { .. },
            ..
        }))
    ));
    assert_eq!(
        app.store
            .load_task_item(&blocked_id)
            .await
            .unwrap()
            .unwrap()
            .depends_on
            .len(),
        1
    );

    app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
        .await
        .unwrap();

    assert!(
        app.store
            .load_task_item(&blocked_id)
            .await
            .unwrap()
            .unwrap()
            .depends_on
            .is_empty()
    );
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&blocker_id)
    );
}

#[tokio::test]
async fn focused_relationship_delete_and_unsupported_actions_are_explicit() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    app.store.refresh(Some(&blocked_id)).await.unwrap();
    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    app.list.select_task(Some(blocked_index));
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::DependsOn,
            task_id: blocker_id.clone(),
        }));

    for code in [KeyCode::Char('t'), KeyCode::Char('r'), KeyCode::Char('e')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("open the related task before using that command")
    );

    for code in [KeyCode::Char('t'), KeyCode::Char('D')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::DeleteFocusedTask { ref selection },
            ..
        })) if selection.single_id() == Some(&blocker_id)
    ));
    assert!(
        !app.store
            .load_task_item(&blocker_id)
            .await
            .unwrap()
            .unwrap()
            .task
            .deleted
    );
}

#[tokio::test]
async fn focused_epic_parent_unlink_requires_confirmation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&child_id)).await.unwrap();
    let child_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == child_id)
        .unwrap();
    app.list.select_task(Some(child_index));
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicParent,
            task_id: parent_id.clone(),
        }));

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('r')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkEpicChild { .. },
            ..
        }))
    ));
    assert!(
        app.store
            .load_task_item(&child_id)
            .await
            .unwrap()
            .unwrap()
            .epic_parent
            .is_some()
    );

    app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
        .await
        .unwrap();

    assert!(
        app.store
            .load_task_item(&child_id)
            .await
            .unwrap()
            .unwrap()
            .epic_parent
            .is_none()
    );
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&child_id)
    );
}

async fn assert_six_child_removal_keyboard_reachability(removal_index: usize) {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, _) = create_epic_with_children(
        &mut app,
        &pool,
        "Parent epic",
        &[
            "Child 0", "Child 1", "Child 2", "Child 3", "Child 4", "Child 5",
        ],
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
    app.show_detail(0);
    let child_ids = app.store.tasks[parent_index]
        .epic_children
        .iter()
        .map(|child| child.task_id.clone())
        .collect::<Vec<_>>();
    let removed_id = child_ids[removal_index].clone();
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: removed_id.clone(),
        }));

    app.remove_selected_epic_child().await.unwrap();

    let size: ratatui::layout::Size = (80, 24).into();
    let collapsed_targets = app.detail_focus_targets(size);
    let collapsed_children = collapsed_targets
        .iter()
        .filter_map(DetailTargetId::task_id)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(collapsed_children, child_ids[..5]);
    assert!(matches!(
        collapsed_targets.last(),
        Some(DetailTargetId::Expand {
            section: DetailSection::EpicChildren
        })
    ));
    let parent = app
        .store
        .selected_task(app.list.selected_task())
        .expect("selected parent epic");
    assert_eq!(parent.epic_children.len(), 5);
    assert!(
        parent
            .epic_children
            .iter()
            .all(|child| child.task_id != removed_id)
    );

    let moves_to_disclosure = if removal_index == 5 {
        app.dispatch_key(key(KeyCode::Tab), size).await.unwrap();
        app.dispatch_key(key(KeyCode::Tab), size).await.unwrap();
        5
    } else {
        5 - removal_index
    };
    for _ in 0..moves_to_disclosure {
        app.dispatch_key(key(KeyCode::Char('j')), size)
            .await
            .unwrap();
    }
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(&DetailTargetId::Expand {
            section: DetailSection::EpicChildren,
        })
    );

    app.dispatch_key(key(KeyCode::Enter), size).await.unwrap();

    assert!(
        app.detail
            .state()
            .unwrap()
            .expanded_sections()
            .contains(&DetailSection::EpicChildren)
    );
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&child_ids[5])
    );
    if removal_index < 5 {
        assert!(
            app.store
                .selected_task(app.list.selected_task())
                .unwrap()
                .epic_children
                .iter()
                .any(|child| child.task_id == child_ids[5])
        );
    }
}

#[tokio::test]
async fn removing_first_of_six_children_keeps_disclosure_and_hidden_child_keyboard_reachable() {
    assert_six_child_removal_keyboard_reachability(0).await;
}

#[tokio::test]
async fn removing_middle_of_six_children_keeps_disclosure_and_hidden_child_keyboard_reachable() {
    assert_six_child_removal_keyboard_reachability(2).await;
}

#[tokio::test]
async fn removing_last_of_six_children_keeps_disclosure_keyboard_reachable() {
    assert_six_child_removal_keyboard_reachability(5).await;
}

#[tokio::test]
async fn focused_detail_child_removes_and_undo_restores_relationship() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (parent_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Parent epic", &["Child task"]).await;
    let child_id = child_ids[0].clone();
    app.store.view_state.view = crate::tui::store::TaskView::Epics;
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.store
        .toggle_selected_epic(Some(parent_index))
        .await
        .unwrap()
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.move_selection(1).await.unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&child_id)
    );
    app.list.select_task(Some(parent_index));
    app.show_detail(0);
    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();

    for code in [KeyCode::Char('t'), KeyCode::Char('c'), KeyCode::Char('r')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkEpicChild { .. },
            ..
        }))
    ));
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_epic_links WHERE epic_task_id = ? AND child_task_id = ?",
    )
    .bind(&parent_id)
    .bind(&child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 1);

    app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .removed_epic_child
            .as_ref()
            .map(|removed| &removed.child.task_id),
        Some(&child_id)
    );
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_epic_links WHERE epic_task_id = ? AND child_task_id = ?",
    )
    .bind(&parent_id)
    .bind(&child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 0);

    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&child_id)
    );

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&parent_id)
    );
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .removed_epic_child
            .as_ref()
            .map(|removed| &removed.child.task_id),
        Some(&child_id)
    );

    app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.detail.state_mut().unwrap().removed_epic_child.is_none());
    assert_eq!(
        app.detail
            .state_mut()
            .unwrap()
            .focused_target
            .as_ref()
            .and_then(DetailTargetId::task_id),
        Some(&child_id)
    );
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_epic_links WHERE epic_task_id = ? AND child_task_id = ?",
    )
    .bind(&parent_id)
    .bind(&child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 1);
}

use super::*;

#[tokio::test]
async fn epic_detail_add_child_shortcut_opens_contextual_search() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
                metadata: Vec::new(),
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
            metadata: Vec::new(),
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
    app.store.show_view(TaskQuery::Epics).await.unwrap();

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
        assert_eq!(toast_message(&app), None);

        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();

        assert!(app.store.view_state.collapsed_epic_ids.contains(&parent_id));
        assert!(!app.store.tasks.iter().any(|item| item.task.id == child_id));
        assert_eq!(toast_message(&app), None);
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
    let (_dir, pool, mut app) = Box::pin(async {
        let (dir, pool, app) = test_app_with_pool().await;
        (dir, pool, Box::new(app))
    })
    .await;
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

async fn setup_related_detail(
    app: &mut App,
    related_count: usize,
) -> (crate::ids::TaskId, Vec<crate::ids::TaskId>) {
    let subject_index = create_and_select_task(app, test_task_draft("Related subject")).await;
    let subject_id = app.store.tasks[subject_index].task.id.clone();
    let mut related_ids = Vec::new();
    for index in 0..related_count {
        let target_index =
            create_and_select_task(app, test_task_draft(&format!("Related target {index}"))).await;
        let target_id = app.store.tasks[target_index].task.id.clone();
        related_ids.push(target_id.clone());
        let subject_index = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == subject_id)
            .unwrap();
        app.store
            .add_related(Some(subject_index), &target_id)
            .await
            .unwrap();
    }
    app.store.refresh(Some(&subject_id)).await.unwrap();
    let subject_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == subject_id)
        .unwrap();
    app.list.select_task(Some(subject_index));
    app.show_detail(0);
    let related_ids = app
        .detail_focus_targets((80, 24).into())
        .into_iter()
        .filter_map(|target| match target {
            DetailTargetId::Task {
                section: DetailSection::Related,
                task_id,
            } => Some(task_id),
            _ => None,
        })
        .collect();
    (subject_id, related_ids)
}

async fn remove_focused_related_row(app: &mut App, related_id: crate::ids::TaskId) {
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::Related,
            task_id: related_id,
        }));
    for code in [KeyCode::Char('t'), KeyCode::Char('k'), KeyCode::Char('r')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert!(matches!(app.overlay, Some(OverlayState::Confirm(_))));
    app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
        .await
        .unwrap();
}

#[tokio::test]
async fn focused_related_middle_removal_focuses_next_row() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (_subject_id, related_ids) = setup_related_detail(&mut app, 3).await;
    remove_focused_related_row(&mut app, related_ids[1].clone()).await;

    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(&DetailTargetId::Task {
            section: DetailSection::Related,
            task_id: related_ids[2].clone(),
        })
    );
    app.dispatch_key(key(KeyCode::Enter), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&related_ids[2])
    );
}

#[tokio::test]
async fn focused_related_last_removal_focuses_previous_row() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (_subject_id, related_ids) = setup_related_detail(&mut app, 3).await;
    remove_focused_related_row(&mut app, related_ids[2].clone()).await;

    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(&DetailTargetId::Task {
            section: DetailSection::Related,
            task_id: related_ids[1].clone(),
        })
    );
}

#[tokio::test]
async fn focused_only_related_removal_reconciles_to_another_detail_target() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (subject_id, related_ids) = setup_related_detail(&mut app, 1).await;
    let dependency_index =
        create_and_select_task(&mut app, test_task_draft("Dependency target")).await;
    let dependency_id = app.store.tasks[dependency_index].task.id.clone();
    let subject_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == subject_id)
        .unwrap();
    app.store
        .add_dependency(Some(subject_index), &dependency_id)
        .await
        .unwrap();
    app.store.refresh(Some(&subject_id)).await.unwrap();
    let subject_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == subject_id)
        .unwrap();
    app.list.select_task(Some(subject_index));
    app.show_detail(0);
    remove_focused_related_row(&mut app, related_ids[0].clone()).await;

    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(&DetailTargetId::Task {
            section: DetailSection::DependsOn,
            task_id: dependency_id,
        })
    );
}

#[tokio::test]
async fn command_panel_unlinks_captured_relationship_a_after_focus_moves_to_b() {
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

    app.dispatch_key(key(KeyCode::Char(':')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    let other = create_and_select_task(&mut app, test_task_draft("Relationship B")).await;
    let other_id = app.store.tasks[other].task.id.clone();
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::Blocks,
            task_id: other_id,
        }));
    type_chars(&mut app, "remove-dependency").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkDependency { ref selection, ref depends_on_task_id },
            ..
        })) if selection.single_id() == Some(&blocked_id) && depends_on_task_id == &blocker_id
    ));
}

#[tokio::test]
async fn command_panel_add_related_keeps_the_captured_parent_task() {
    let mut app = test_app().await;
    let subject_index =
        create_and_select_task(&mut app, test_task_draft("Related command subject")).await;
    let subject_id = app.store.tasks[subject_index].task.id.clone();
    let subject_ref = app.store.tasks[subject_index].display_ref.clone();
    let other_index =
        create_and_select_task(&mut app, test_task_draft("Different live selection")).await;
    app.list.select_task(Some(subject_index));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    app.list.select_task(Some(other_index));
    type_chars(&mut app, "add-related").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(SearchState {
            intent: SearchIntent::AddRelated { selection, display_ref },
            ..
        })) if selection.single_id() == Some(&subject_id) && display_ref == &subject_ref
    ));
}

#[tokio::test]
async fn command_panel_ranks_and_unlinks_the_captured_related_task() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (subject_id, related_ids) = setup_related_detail(&mut app, 2).await;
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::Related,
            task_id: related_ids[0].clone(),
        }));

    app.dispatch_key(key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("command overlay");
    };
    let names = state
        .candidates
        .iter()
        .filter_map(|candidate| state.catalog.command(candidate.index))
        .map(crate::tui::event::CatalogCommand::name)
        .collect::<Vec<_>>();
    assert_eq!(
        &names[..8],
        &[
            "status-picker",
            "status-done",
            "edit-title",
            "copy-ref",
            "copy-title",
            "remove-related",
            "delete",
            "add-note",
        ]
    );

    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::Related,
            task_id: related_ids[1].clone(),
        }));
    type_chars(&mut app, "remove-related").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkRelated { ref selection, ref related_task_id },
            ..
        })) if selection.single_id() == Some(&subject_id) && related_task_id == &related_ids[0]
    ));
}

#[tokio::test]
async fn command_panel_targets_the_captured_epic_child_and_keeps_the_parent_anchor() {
    let mut app = test_app().await;
    let parent_index = create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            is_epic: true,
            ..test_task_draft("Parent epic")
        },
    )
    .await;
    let parent_id = app.store.tasks[parent_index].task.id.clone();
    let parent_ref = app.store.tasks[parent_index].display_ref.clone();
    let project_key = app.store.tasks[parent_index].task.project_key.clone();
    let child_index = create_and_select_task(&mut app, test_task_draft("Captured child")).await;
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
    app.store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    app.list.select_task(Some(parent_index));
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: child_id.clone(),
        }));

    app.dispatch_key(key(KeyCode::Char(':')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    app.detail.state_mut().unwrap().set_focused_target(None);
    type_chars(&mut app, "edit-title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(
                &state.intent,
                TextIntent::EditTitle { selection }
                    if selection.single_id() == Some(&child_id)
            )
    ));
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&parent_id)
    );
    assert!(app.detail.is_active());
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
        Some("invalid shortcut: t r")
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
    app.store.view_state.query = crate::tui::store::TaskQuery::Epics;
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

#[tokio::test]
async fn colon_dispatch_opens_captured_dependency_relationship_panel() {
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

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();

    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("command overlay");
    };
    let names = state
        .candidates
        .iter()
        .filter_map(|candidate| state.catalog.command(candidate.index))
        .map(crate::tui::event::CatalogCommand::name)
        .collect::<Vec<_>>();
    assert_eq!(
        &names[..8],
        &[
            "status-picker",
            "status-done",
            "edit-title",
            "copy-ref",
            "copy-title",
            "remove-dependency",
            "delete",
            "add-note",
        ]
    );
    assert!(matches!(
        state.session.detail_focus(),
        Some(crate::tui::event::DetailCommandFocus::Relationship { task_id, .. })
            if task_id == &blocker_id
    ));
}

#[tokio::test]
async fn colon_dispatch_opens_captured_epic_child_panel_and_confirms_unlink() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (epic_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Dispatch epic", &["Dispatch child"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&epic_id)).await.unwrap();
    let epic_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == epic_id)
        .unwrap();
    app.list.select_task(Some(epic_index));
    app.show_detail(4);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: child_id.clone(),
        }));

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("command overlay");
    };
    let names = state
        .candidates
        .iter()
        .filter_map(|candidate| state.catalog.command(candidate.index))
        .map(crate::tui::event::CatalogCommand::name)
        .collect::<Vec<_>>();
    assert_eq!(
        &names[..8],
        &[
            "status-picker",
            "status-done",
            "edit-title",
            "copy-ref",
            "copy-title",
            "task-child-remove",
            "delete",
            "add-note",
        ]
    );

    type_chars(&mut app, "task-child-remove").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkEpicChild { target, .. },
            ..
        })) if target.epic.epic_id == epic_id && target.child.task_id == child_id
    ));
}

#[tokio::test]
async fn command_panel_rejects_epic_removal_for_dependency_relationship() {
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
            task_id: blocker_id,
        }));

    app.begin_command().await;
    type_chars(&mut app, "task-child-remove").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(!matches!(app.overlay, Some(OverlayState::Confirm(_))));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":task-child-remove is disabled: command does not apply to the captured relationship")
    );
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
}

#[tokio::test]
async fn parent_detail_child_remove_command_requires_confirmation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (epic_id, child_ids) = create_epic_with_children(
        &mut app,
        &pool,
        "Parent command epic",
        &["Parent command child"],
    )
    .await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&child_id)).await.unwrap();
    app.list.select_task(Some(0));
    app.show_detail(0);

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "task-child-remove").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkEpicChild { target, .. },
            ..
        })) if target.epic.epic_id == epic_id && target.child.task_id == child_id
    ));
    assert_eq!(
        app.store
            .load_task_item(&child_id)
            .await
            .unwrap()
            .unwrap()
            .epic_parent
            .as_ref()
            .map(|parent| &parent.task_id),
        Some(&epic_id)
    );
}

#[tokio::test]
async fn command_panel_rejects_dependency_removal_for_epic_child() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (epic_id, child_ids) =
        create_epic_with_children(&mut app, &pool, "Cross pair epic", &["Epic child"]).await;
    let child_id = child_ids[0].clone();
    app.store.refresh(Some(&epic_id)).await.unwrap();
    let epic_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == epic_id)
        .unwrap();
    app.list.select_task(Some(epic_index));
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: child_id.clone(),
        }));

    app.begin_command().await;
    type_chars(&mut app, "remove-dependency").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(!matches!(app.overlay, Some(OverlayState::Confirm(_))));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":remove-dependency is disabled: command does not apply to the captured relationship")
    );
    assert_eq!(
        app.store
            .load_task_item(&child_id)
            .await
            .unwrap()
            .unwrap()
            .epic_parent
            .as_ref()
            .map(|parent| &parent.task_id),
        Some(&epic_id)
    );
}

#[tokio::test]
async fn epic_child_confirmation_uses_captured_target_after_focus_changes() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let (epic_id, child_ids) = create_epic_with_children(
        &mut app,
        &pool,
        "Captured confirmation epic",
        &["Captured child", "Other child"],
    )
    .await;
    let captured_child_id = child_ids[0].clone();
    let other_child_id = child_ids[1].clone();
    app.store.refresh(Some(&epic_id)).await.unwrap();
    let epic_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == epic_id)
        .unwrap();
    app.list.select_task(Some(epic_index));
    app.show_detail(7);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: captured_child_id.clone(),
        }));

    app.begin_command().await;
    type_chars(&mut app, "task-child-remove").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Confirm(_))));

    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: other_child_id.clone(),
        }));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    let epic = app.store.load_task_item(&epic_id).await.unwrap().unwrap();
    assert!(
        epic.epic_children
            .iter()
            .all(|child| child.task_id != captured_child_id)
    );
    assert!(
        epic.epic_children
            .iter()
            .any(|child| child.task_id == other_child_id)
    );
    assert_eq!(app.detail.state().unwrap().scroll(), 7);
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .and_then(DetailTargetId::task_id),
        Some(&captured_child_id)
    );
}

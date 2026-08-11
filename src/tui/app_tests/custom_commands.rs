use std::path::PathBuf;
use std::time::Duration;

use crate::config::{
    AppConfig, CustomTuiCommandConfig, CustomTuiCommandExecution, CustomTuiCommandSuccess,
    CustomTuiCommandTarget,
};

use super::*;

fn config(program: &str, success: CustomTuiCommandSuccess) -> AppConfig {
    let mut config = AppConfig::default();
    config.tui.commands.push(CustomTuiCommandConfig {
        name: "dispatch".to_string(),
        aliases: vec!["custom-dispatch".to_string()],
        description: "dispatch selected task".to_string(),
        program: PathBuf::from(program),
        args: (program == "/usr/bin/tee")
            .then(|| "/dev/null".to_string())
            .into_iter()
            .collect(),
        keys: vec![],
        detail_keys: None,
        target: CustomTuiCommandTarget::None,
        execution: CustomTuiCommandExecution::Wait,
        on_success: success,
    });
    config
}

fn capture_config(
    path: &std::path::Path,
    target: CustomTuiCommandTarget,
    keys: Vec<String>,
) -> AppConfig {
    let mut config = config("/usr/bin/tee", CustomTuiCommandSuccess::Stay);
    let command = &mut config.tui.commands[0];
    command.args = vec![path.to_string_lossy().into_owned()];
    command.target = target;
    command.keys = keys;
    config
}

fn captured_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

async fn poll_until_complete(app: &mut App) {
    for _ in 0..100 {
        if app.poll_custom_commands().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("custom command did not complete");
}

#[tokio::test]
async fn configured_keybinding_executes_custom_command() {
    let mut app = test_app().await;
    let mut app_config = config("/usr/bin/tee", CustomTuiCommandSuccess::Stay);
    app_config.tui.commands[0].keys = vec!["z d".to_string()];
    app.set_config(app_config);

    app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
    assert!(!app.pending_shortcut.is_empty());
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert!(app.pending_shortcut.is_empty());
    assert!(toast_message(&app).unwrap().contains("running :dispatch"));

    poll_until_complete(&mut app).await;
    assert!(toast_message(&app).unwrap().contains("completed"));
}

#[tokio::test]
async fn detail_keybinding_override_executes_custom_command() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail task")).await;
    let mut app_config = config("/usr/bin/tee", CustomTuiCommandSuccess::Stay);
    app_config.tui.commands[0].keys = vec!["z d".to_string()];
    app_config.tui.commands[0].detail_keys = Some(vec!["Z".to_string()]);
    app.set_config(app_config);
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('Z')), (80, 24).into())
        .await
        .unwrap();
    assert!(toast_message(&app).unwrap().contains("running :dispatch"));

    poll_until_complete(&mut app).await;
    assert!(app.detail.is_active());
    assert!(toast_message(&app).unwrap().contains("completed"));
}

#[tokio::test]
async fn successful_waiting_command_applies_quit_policy() {
    let mut app = test_app().await;
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Quit));

    app.execute_custom_command(0, "custom-dispatch")
        .await
        .unwrap();
    assert!(!app.should_quit);
    poll_until_complete(&mut app).await;

    assert!(app.should_quit);
}

#[tokio::test]
async fn failed_waiting_command_leaves_app_open_with_error() {
    let mut app = test_app().await;
    app.set_config(config("/usr/bin/false", CustomTuiCommandSuccess::Quit));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;

    assert!(!app.should_quit);
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(toast_message(&app).unwrap().contains(":dispatch"));
}

#[tokio::test]
async fn successful_stay_policy_leaves_app_open() {
    let mut app = test_app().await;
    app.set_config(config("/usr/bin/tee", CustomTuiCommandSuccess::Stay));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;

    assert!(!app.should_quit);
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
    assert!(toast_message(&app).unwrap().contains("completed"));
}

#[tokio::test]
async fn target_none_is_available_without_primary_in_list_and_detail_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let list_path = dir.path().join("list.json");
    let detail_path = dir.path().join("detail.json");
    let mut app = test_app().await;
    app.list.select_task(None);
    app.set_config(capture_config(
        &list_path,
        CustomTuiCommandTarget::None,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let list = captured_json(&list_path);
    assert!(list["selection"]["primary"].is_null());
    assert_eq!(list["targeting"]["targets"], serde_json::json!([]));

    create_and_select_task(&mut app, test_task_draft("detail primary")).await;
    app.show_detail(0);
    app.set_config(capture_config(
        &detail_path,
        CustomTuiCommandTarget::None,
        vec![],
    ));
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let detail = captured_json(&detail_path);
    assert!(detail["selection"]["primary"].is_object());
    assert_eq!(detail["targeting"]["resolved_from"], "none");
}

#[tokio::test]
async fn focused_target_uses_list_or_displayed_detail_primary_and_requires_one() {
    let dir = tempfile::tempdir().unwrap();
    let list_path = dir.path().join("list.json");
    let detail_path = dir.path().join("detail.json");
    let mut app = test_app().await;
    app.list.select_task(None);
    app.set_config(capture_config(
        &list_path,
        CustomTuiCommandTarget::Focused,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":dispatch is disabled: requires a focused task")
    );
    assert!(!list_path.exists());

    let selected = create_and_select_task(&mut app, test_task_draft("displayed primary")).await;
    let primary_id = app.store.tasks[selected].task.id.clone();
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    assert_eq!(
        captured_json(&list_path)["targeting"]["targets"][0]["id"],
        primary_id.as_str()
    );

    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: crate::test_support::task_id("related-focus"),
        }));
    app.set_config(capture_config(
        &detail_path,
        CustomTuiCommandTarget::Focused,
        vec![],
    ));
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    assert_eq!(
        captured_json(&detail_path)["targeting"]["targets"][0]["id"],
        primary_id.as_str()
    );
}

#[tokio::test]
async fn marked_target_requires_marks_and_preserves_visible_order_in_list_and_detail() {
    let dir = tempfile::tempdir().unwrap();
    let list_path = dir.path().join("list.json");
    let detail_path = dir.path().join("detail.json");
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first mark")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second mark")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.set_config(capture_config(
        &list_path,
        CustomTuiCommandTarget::Marked,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":dispatch is disabled: requires one or more marked tasks")
    );

    app.list.mark(second_id);
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let one = captured_json(&list_path);
    assert_eq!(one["targeting"]["targets"].as_array().unwrap().len(), 1);

    app.list.mark(first_id);
    let expected = app
        .store
        .tasks
        .iter()
        .filter(|item| app.list.marked_task_ids().contains(&item.task.id))
        .map(|item| item.task.id.to_string())
        .collect::<Vec<_>>();
    app.set_config(capture_config(
        &detail_path,
        CustomTuiCommandTarget::Marked,
        vec![],
    ));
    app.show_detail(0);
    app.begin_command().await;
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.marked_task_count == 0 && state.custom_command_marked_task_count == 2
    ));
    app.overlay = None;
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let detail = captured_json(&detail_path);
    let actual = detail["targeting"]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(detail["selection"]["marked"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn marked_or_focused_falls_back_then_prefers_marks() {
    let dir = tempfile::tempdir().unwrap();
    let fallback_path = dir.path().join("fallback.json");
    let marked_path = dir.path().join("marked.json");
    let mut app = test_app().await;
    let primary = create_and_select_task(&mut app, test_task_draft("primary")).await;
    let primary_id = app.store.tasks[primary].task.id.clone();
    let marked = create_and_select_task(&mut app, test_task_draft("marked")).await;
    let marked_id = app.store.tasks[marked].task.id.clone();
    let primary = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == primary_id)
        .unwrap();
    app.list.select_task(Some(primary));
    app.set_config(capture_config(
        &fallback_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec![],
    ));

    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let fallback = captured_json(&fallback_path);
    assert_eq!(fallback["targeting"]["resolved_from"], "focused");
    assert_eq!(
        fallback["targeting"]["targets"][0]["id"],
        primary_id.as_str()
    );

    app.list.mark(marked_id.clone());
    app.set_config(capture_config(
        &marked_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec![],
    ));
    app.execute_custom_command(0, "dispatch").await.unwrap();
    poll_until_complete(&mut app).await;
    let preferred = captured_json(&marked_path);
    assert_eq!(preferred["targeting"]["resolved_from"], "marked");
    assert_eq!(
        preferred["targeting"]["targets"][0]["id"],
        marked_id.as_str()
    );
}

#[tokio::test]
async fn palette_and_shortcut_invocations_produce_identical_targets() {
    let dir = tempfile::tempdir().unwrap();
    let palette_path = dir.path().join("palette.json");
    let shortcut_path = dir.path().join("shortcut.json");
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id);
    app.list.mark(second_id);
    app.set_config(capture_config(
        &palette_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec!["z d".to_string()],
    ));

    app.begin_command().await;
    for ch in "dispatch".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    poll_until_complete(&mut app).await;

    app.set_config(capture_config(
        &shortcut_path,
        CustomTuiCommandTarget::MarkedOrFocused,
        vec!["z d".to_string()],
    ));
    app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    poll_until_complete(&mut app).await;

    let palette = captured_json(&palette_path);
    let shortcut = captured_json(&shortcut_path);
    assert_eq!(palette["targeting"], shortcut["targeting"]);
    assert_eq!(palette["selection"], shortcut["selection"]);
}

use std::path::PathBuf;
use std::time::Duration;

use crate::config::{
    AppConfig, CustomTuiCommandConfig, CustomTuiCommandExecution, CustomTuiCommandRequirement,
    CustomTuiCommandSuccess,
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
        requires: CustomTuiCommandRequirement::None,
        execution: CustomTuiCommandExecution::Wait,
        on_success: success,
    });
    config
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

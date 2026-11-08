use super::*;

#[tokio::test]
async fn task_creation_wakes_configured_daemon() {
    let mut store = test_store().await;
    let host = crate::host::wake::Listener::bind(store.database().path());
    let mut config = crate::config::AppConfig::default();
    config.sync.enabled = true;
    store.set_config(config);

    create_selected_task(&mut store, "wake the daemon").await;

    let mut buf = [0_u8; 1];
    assert_eq!(host.try_recv(&mut buf).unwrap(), 1);
    assert_eq!(buf, [b'1']);

    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.title == "wake the daemon")
        .unwrap();
    store.update_status(Some(selected), "todo").await.unwrap();
    assert_eq!(host.try_recv(&mut buf).unwrap(), 1);
    assert_eq!(buf, [b'1']);
}

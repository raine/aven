use super::*;

#[tokio::test]
async fn task_creation_wakes_configured_daemon() {
    let mut store = test_store().await;
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    let mut config = crate::config::AppConfig::default();
    config.sync.enabled = true;
    config.daemon.wake_addr = Some(socket.local_addr().unwrap().to_string());
    store.set_config(config);

    create_selected_task(&mut store, "wake the daemon").await;

    let mut buf = [0_u8; 1];
    assert_eq!(socket.recv(&mut buf).unwrap(), 1);
    assert_eq!(buf, [b'1']);

    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.title == "wake the daemon")
        .unwrap();
    store.update_status(Some(selected), "todo").await.unwrap();
    assert_eq!(socket.recv(&mut buf).unwrap(), 1);
    assert_eq!(buf, [b'1']);
}

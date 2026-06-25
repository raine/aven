use anyhow::Result;
use sqlx::SqliteConnection;

use crate::config as app_config;
use crate::db::get_meta;
use crate::operations::{
    init_config as init_config_operation, show_config as show_config_operation,
    show_config_paths as show_config_paths_operation,
};

use super::TuiStore;
use super::types::{SyncStatusCheck, TuiSyncStatus};

impl TuiStore {
    pub(crate) fn config_info_lines(&self) -> Result<Vec<String>> {
        let outcome = show_config_operation()?;
        let mut lines = vec![
            format!("config path: {}", outcome.path.display()),
            String::new(),
        ];
        lines.extend(outcome.text.lines().map(str::to_string));
        Ok(lines)
    }

    pub(crate) fn config_path_lines(&self) -> Result<Vec<String>> {
        Ok(show_config_paths_operation()?.lines)
    }

    pub(crate) fn init_config(&self) -> Result<String> {
        let outcome = init_config_operation()?;
        Ok(format!("created config {}", outcome.path.display()))
    }

    pub(super) async fn load_sync_status(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<TuiSyncStatus> {
        let config = match app_config::AppConfig::load() {
            Ok(config) => config,
            Err(error) => {
                return Ok(TuiSyncStatus {
                    config_error: Some(format!("{error:#}")),
                    ..TuiSyncStatus::default()
                });
            }
        };
        let pinned_server = get_meta(conn, "sync_server_url").await?;
        let configured_server = configured_server_check(&config);
        let server_match = configured_server
            .as_ref()
            .filter(|server| server.ok)
            .and_then(|server| {
                pinned_server.as_deref().map(|pinned| {
                    let configured = server.value.trim_end_matches('/');
                    SyncStatusCheck::new(
                        pinned == configured,
                        if pinned == configured {
                            "yes".to_string()
                        } else {
                            format!("pinned={pinned} configured={configured}")
                        },
                    )
                })
            });
        let daemon_server = daemon_server_check(&config);
        let daemon_wake = match config.wake_addr() {
            Ok(addr) => SyncStatusCheck::new(true, addr.to_string()),
            Err(error) => SyncStatusCheck::new(false, format!("{error:#}")),
        };
        let pending_changes: i64 =
            sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await?;
        let conflicts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM conflicts WHERE resolved = 0")
                .fetch_one(&mut *conn)
                .await?;

        Ok(TuiSyncStatus {
            enabled: config.sync.enabled,
            config_error: None,
            configured_server,
            pinned_server,
            server_match,
            daemon_server,
            auth_token_configured: config.sync_auth_token().is_some(),
            interval_seconds: config.sync_interval_seconds(),
            daemon_wake,
            pending_changes,
            conflicts,
            sync_cursor: get_meta(conn, "sync_cursor").await?,
            local_sequence: get_meta(conn, "local_seq").await?,
            last_attempt: get_meta(conn, "sync_last_attempt_at").await?,
            last_success: get_meta(conn, "sync_last_success_at").await?,
            last_error: get_meta(conn, "sync_last_error").await?,
            last_pushed: get_meta(conn, "sync_last_pushed").await?,
            last_pulled: get_meta(conn, "sync_last_pulled").await?,
            last_cursor: get_meta(conn, "sync_last_cursor").await?,
        })
    }
}

fn configured_server_check(config: &app_config::AppConfig) -> Option<SyncStatusCheck> {
    match app_config::resolve_sync_server(None, config) {
        Ok(server) => Some(SyncStatusCheck::new(
            sync_server_url_is_valid(&server),
            server,
        )),
        Err(error) if config.sync.enabled => {
            Some(SyncStatusCheck::new(false, format!("{error:#}")))
        }
        Err(_) => None,
    }
}

fn daemon_server_check(config: &app_config::AppConfig) -> Option<SyncStatusCheck> {
    match config.sync.server_url.as_deref() {
        Some(server) => Some(SyncStatusCheck::new(
            sync_server_url_is_valid(server),
            server,
        )),
        None if config.sync.enabled => Some(SyncStatusCheck::new(false, "not configured")),
        None => None,
    }
}

fn sync_server_url_is_valid(server: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

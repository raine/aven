//! Host logic shared by the CLI, TUI and desktop app: configuration and path
//! policy, routing, input parsing, task intake, the desktop side of sync, and the task store.
use aven_core::{choices, ids, query, queue, types, undo};

#[cfg(target_os = "macos")]
pub mod app_bundle;
pub mod attachments;
pub mod background;
pub mod config;
pub mod config_edit;
pub mod daemon;
#[cfg(target_os = "macos")]
pub mod desktop_install;
pub mod due;
pub mod gist;
pub mod host;
pub mod notify;
pub mod operations;
mod paths;
pub mod projects;
pub mod protected_local_keys;
pub mod recurrence_input;
pub mod reminders;
pub mod render;
pub mod routing;
#[cfg(target_os = "macos")]
pub mod running_processes;
pub mod schedule_input;
pub mod status;
pub mod store;
pub mod sync;
pub mod sync_http;
pub mod task_intake;
pub mod task_render;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod time_input;
pub mod workspaces;

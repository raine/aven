//! Host logic shared by the CLI, TUI and desktop app: configuration and path
//! policy, routing, input parsing and task intake.
use aven_core::{choices, ids, query, types};

pub mod attachments;
pub mod config;
pub mod config_edit;
pub mod operations;
pub mod projects;
pub mod recurrence_input;
pub mod render;
pub mod routing;
pub mod schedule_input;
pub mod task_intake;
pub mod task_render;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod time_input;
pub mod workspaces;

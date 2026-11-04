//! The Aven desktop app, a GPUI application on the shared Aven crates. It exists
//! only on macOS, where `aven` links it behind the `desktop` feature.
#![cfg(target_os = "macos")]

mod actions;
pub mod adapter;
mod app;
mod assets;
mod board;
mod bounce;
mod commands;
mod composer;
mod composer_host;
mod details;
mod dialogs;
mod display;
mod draft;
mod images;
mod input;
pub mod intro;
mod kit;
mod lazy_row;
mod ledger;
mod markdown;
mod overlays;
mod platform;
mod schedule;
pub mod session;
mod settings;
mod shell;
mod startup;
mod switch;
mod sync_ops;
pub mod theme;
pub mod toast;
mod welcome;
mod workflows;

/// Starts the app and returns when it quits. Takes over the main thread, and
/// must run outside any async runtime.
pub fn launch() -> anyhow::Result<()> {
    app::run()
}

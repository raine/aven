//! The Aven desktop app, a GPUI application on the shared Aven crates. It exists
//! only on macOS, where `aven` links it behind the `desktop` feature.
#![cfg(target_os = "macos")]

pub mod actions;
pub mod adapter;
pub mod app;
pub mod assets;
mod board;
pub mod bounce;
pub mod capabilities;
mod commands;
mod composer;
mod details;
mod dialogs;
pub mod draft;
#[cfg(feature = "capture")]
pub mod fixture;
pub mod input;
#[cfg(feature = "dev")]
pub mod isolation;
pub mod kit;
pub mod markdown;
pub mod measure;
mod overlays;
mod platform;
#[cfg(feature = "capture")]
pub mod prototype;
#[cfg(feature = "capture")]
pub mod scene;
pub mod session;
mod settings;
pub mod shell;
pub mod sync_ops;
pub mod theme;
mod workflows;

/// Starts the app and returns when it quits. Takes over the main thread, and
/// must run outside any async runtime.
#[cfg(not(feature = "dev"))]
pub fn launch() -> anyhow::Result<()> {
    app::run(app::Start::installed()?)
}

/// Starts the app on the disposable root named by the environment, or refuses.
#[cfg(feature = "dev")]
pub fn launch() -> anyhow::Result<()> {
    app::run(app::Start::isolated(isolation::Isolation::from_process()?)?)
}

/// Refuses a path a development launch must not reach. An installed app works
/// on the owner's own files, so there it allows everything.
fn require_allowed(name: &str, path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(feature = "dev")]
    isolation::require_outside_owner(name, path)?;
    #[cfg(not(feature = "dev"))]
    let _ = (name, path);
    Ok(())
}

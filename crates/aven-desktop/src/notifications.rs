//! Whether macOS lets Aven show notifications, and asking for it.
//!
//! Only the app asks: the daemon has no window to explain the prompt from.
use std::sync::Mutex;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
};
use tokio::sync::oneshot;

/// What the user allowed in System Settings > Notifications.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Permission {
    Allowed,
    Denied,
    /// Aven hasn't asked yet.
    NotAsked,
    /// Not known: not read yet, or not running from an app bundle, which has
    /// no notification settings.
    #[default]
    Unavailable,
}

/// Where the user changes Aven's notification settings.
pub const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=fi.zendit.Aven";

/// `UNUserNotificationCenter` aborts the process outside an app bundle, as in
/// `cargo run`.
fn center() -> Option<objc2::rc::Retained<UNUserNotificationCenter>> {
    NSBundle::mainBundle().bundleIdentifier()?;
    Some(UNUserNotificationCenter::currentNotificationCenter())
}

pub async fn permission() -> Permission {
    let Some(center) = center() else {
        return Permission::Unavailable;
    };
    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    let completion = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
        // SAFETY: the settings object is valid for the duration of the call.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        if let Some(sender) = sender.lock().ok().and_then(|mut sender| sender.take()) {
            let _ = sender.send(status);
        }
    });
    center.getNotificationSettingsWithCompletionHandler(&completion);
    match receiver.await {
        Ok(UNAuthorizationStatus::NotDetermined) => Permission::NotAsked,
        Ok(UNAuthorizationStatus::Denied) => Permission::Denied,
        Ok(_) => Permission::Allowed,
        Err(_) => Permission::Unavailable,
    }
}

/// Asks the user to allow notifications, once: macOS answers from the
/// recorded choice after that. Returns the resulting permission.
pub async fn request() -> Permission {
    let Some(center) = center() else {
        return Permission::Unavailable;
    };
    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        // SAFETY: when non-null, `error` is valid for the duration of the call.
        if let Some(error) = unsafe { error.as_ref() } {
            tracing::warn!(error = %error.localizedDescription(), "notification permission");
        }
        if let Some(sender) = sender.lock().ok().and_then(|mut sender| sender.take()) {
            let _ = sender.send(granted.as_bool());
        }
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &completion,
    );
    match receiver.await {
        Ok(true) => Permission::Allowed,
        Ok(false) => permission().await,
        Err(_) => Permission::Unavailable,
    }
}

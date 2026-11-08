//! `UNUserNotificationCenter` in this process.
//!
//! Talking to it connects this process to the notification service for as
//! long as it runs, and a connected process takes clicks on Aven's
//! notifications from the app. Only the app and the short-lived child of
//! [`super::child`] use this.
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSArray, NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationRequest, UNNotificationSettings, UNUserNotificationCenter,
};

use super::{ASK_TIMEOUT, Authorization, Deliver, Report, Request};

/// How long one call waits for the notification service to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Delivers from this process.
pub struct InProcess;

impl Deliver for InProcess {
    fn deliver(&self, request: &Request) -> Result<Report> {
        let Some(center) = center() else {
            return Ok(Report::not_allowed(Authorization::Unavailable));
        };
        let mut authorization = authorization_of(&center)?;
        if request.ask && authorization == Authorization::NotAsked {
            authorization = ask(&center)?;
        }
        if authorization != Authorization::Allowed {
            return Ok(Report::not_allowed(authorization));
        }
        let delivered = if request.list_delivered {
            Some(delivered(&center)?)
        } else {
            None
        };
        let errors = request
            .posts
            .iter()
            .map(|post| post_one(&center, &post.tag, &post.title, &post.body))
            .collect::<Result<_>>()?;
        Ok(Report {
            authorization,
            errors,
            delivered,
        })
    }
}

/// `UNUserNotificationCenter` aborts a process outside an app bundle, as in
/// `cargo run`.
fn center() -> Option<Retained<UNUserNotificationCenter>> {
    NSBundle::mainBundle().bundleIdentifier()?;
    Some(UNUserNotificationCenter::currentNotificationCenter())
}

fn authorization_of(center: &UNUserNotificationCenter) -> Result<Authorization> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
        // SAFETY: the settings object is valid for the duration of the call.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = sender.try_send(status);
    });
    center.getNotificationSettingsWithCompletionHandler(&completion);
    Ok(match answer(&receiver, "notification settings")? {
        UNAuthorizationStatus::NotDetermined => Authorization::NotAsked,
        UNAuthorizationStatus::Denied => Authorization::Denied,
        _ => Authorization::Allowed,
    })
}

/// Asks the user to allow notifications and waits for the answer.
fn ask(center: &UNUserNotificationCenter) -> Result<Authorization> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(move |_granted: Bool, error: *mut NSError| {
        // SAFETY: when non-null, `error` is valid for the duration of the call.
        let error = unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
        let _ = sender.try_send(error);
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &completion,
    );
    let error = receiver
        .recv_timeout(ASK_TIMEOUT)
        .map_err(|_| anyhow!("no answer to the notification question"))?;
    if let Some(error) = error {
        tracing::debug!(%error, "notification permission");
    }
    authorization_of(center)
}

fn delivered(center: &UNUserNotificationCenter) -> Result<Vec<String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(
        move |notifications: std::ptr::NonNull<NSArray<UNNotification>>| {
            // SAFETY: the array is valid for the duration of the call.
            let notifications = unsafe { notifications.as_ref() };
            let tags = (0..notifications.count())
                .map(|index| {
                    let notification = notifications.objectAtIndex(index);
                    notification.request().identifier().to_string()
                })
                .collect::<Vec<_>>();
            let _ = sender.try_send(tags);
        },
    );
    center.getDeliveredNotificationsWithCompletionHandler(&completion);
    answer(&receiver, "delivered notifications")
}

/// Posts one notification. The outer error means no answer came; the inner
/// one is the service's.
fn post_one(
    center: &UNUserNotificationCenter,
    tag: &str,
    title: &str,
    body: &str,
) -> Result<Option<String>> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    // A nil trigger delivers now.
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(tag),
        &content,
        None,
    );
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: when non-null, `error` is valid for the duration of the call.
        let error = unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
        let _ = sender.try_send(error);
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&completion));
    answer(&receiver, "the posted notification")
}

fn answer<T>(receiver: &mpsc::Receiver<T>, what: &str) -> Result<T> {
    receiver
        .recv_timeout(ANSWER_TIMEOUT)
        .map_err(|_| anyhow!("the notification service did not answer about {what}"))
}

//! The freedesktop notification service on the session bus, for a process
//! that stays running to hear clicks.
//!
//! The service tells the connection that posted a notification when it is
//! clicked; KDE tells no other connection. So one connection posts and
//! listens, for as long as this process runs. A click on the body is the
//! notification's `default` action, and comes with an activation token
//! when the service has one.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;

use super::{Authorization, Deliver, Post, Report, Request};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The action a click on the notification's body invokes.
const DEFAULT_ACTION: &str = "default";
/// The most notifications remembered for clicks, for services that never
/// say a notification closed.
const REMEMBERED: usize = 256;

/// A click on one of this process's notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Click {
    pub tag: String,
    /// The token that lets the window the click opens take focus: an
    /// xdg-activation token on Wayland, a startup id on X11.
    pub activation_token: Option<String>,
}

type OnClick = Arc<dyn Fn(Click) + Send + Sync>;

/// Posts notifications and hands clicks on them to a callback, on a thread
/// of its own.
pub struct Freedesktop {
    app_name: String,
    on_click: OnClick,
    bus: Mutex<Option<Bus>>,
}

struct Bus {
    connection: Connection,
    /// The tag of each notification the service shows, by its id.
    shown: Arc<Mutex<HashMap<u32, String>>>,
}

impl Freedesktop {
    /// Connects on the first delivery, and again on the next one after a
    /// failed connection.
    pub fn new(app_name: &str, on_click: impl Fn(Click) + Send + Sync + 'static) -> Self {
        Self {
            app_name: app_name.to_string(),
            on_click: Arc::new(on_click),
            bus: Mutex::new(None),
        }
    }

    fn post(&self, post: &Post) -> Result<()> {
        let mut guard = self.bus.lock().unwrap_or_else(PoisonError::into_inner);
        let bus = match &mut *guard {
            Some(bus) => bus,
            empty => empty.insert(connect(self.on_click.clone())?),
        };
        let hints: HashMap<&str, Value<'_>> = HashMap::new();
        let reply = bus.connection.call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "Notify",
            &(
                self.app_name.as_str(),
                0_u32,
                "",
                post.title.as_str(),
                post.body.as_str(),
                [DEFAULT_ACTION, "Open"].as_slice(),
                hints,
                -1_i32,
            ),
        );
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                // The next post connects again, in case the bus went away.
                *guard = None;
                return Err(error.into());
            }
        };
        let id: u32 = reply.body().deserialize()?;
        let mut shown = bus.shown.lock().unwrap_or_else(PoisonError::into_inner);
        if shown.len() >= REMEMBERED
            && let Some(oldest) = shown.keys().min().copied()
        {
            shown.remove(&oldest);
        }
        shown.insert(id, post.tag.clone());
        Ok(())
    }
}

impl Deliver for Freedesktop {
    /// The service has no permission to read or list of what it shows.
    fn deliver(&self, request: &Request) -> Result<Report> {
        let errors = request
            .posts
            .iter()
            .map(|post| self.post(post).err().map(|error| format!("{error:#}")))
            .collect();
        Ok(Report {
            authorization: Authorization::Allowed,
            errors,
            delivered: None,
        })
    }
}

/// Connects to the session bus and listens there for the service's signals
/// before anything is posted.
fn connect(on_click: OnClick) -> Result<Bus> {
    let connection = Connection::session().context("connect to the session bus")?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(SERVICE)?
        .interface(SERVICE)?
        .path(PATH)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &connection, None)?;
    let shown = Arc::new(Mutex::new(HashMap::new()));
    let listening = shown.clone();
    std::thread::Builder::new()
        .name("aven-notification-clicks".into())
        .spawn(move || listen(signals, &listening, on_click.as_ref()))?;
    Ok(Bus { connection, shown })
}

/// What the service said about a notification.
#[derive(Debug, PartialEq, Eq)]
enum Signal {
    /// Comes before the action it lets take focus.
    ActivationToken(u32, String),
    ActionInvoked(u32, String),
    Closed(u32),
}

fn listen(
    signals: MessageIterator,
    shown: &Mutex<HashMap<u32, String>>,
    on_click: &(dyn Fn(Click) + Send + Sync),
) {
    let mut tokens = HashMap::new();
    for message in signals.flatten() {
        let header = message.header();
        let Some(member) = header.member() else {
            continue;
        };
        let body = message.body();
        let signal = match member.as_str() {
            "ActivationToken" => body
                .deserialize::<(u32, String)>()
                .map(|(id, token)| Signal::ActivationToken(id, token)),
            "ActionInvoked" => body
                .deserialize::<(u32, String)>()
                .map(|(id, action)| Signal::ActionInvoked(id, action)),
            "NotificationClosed" => body
                .deserialize::<(u32, u32)>()
                .map(|(id, _reason)| Signal::Closed(id)),
            _ => continue,
        };
        let Ok(signal) = signal else {
            continue;
        };
        let mut shown = shown.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(click) = follow(signal, &mut shown, &mut tokens) {
            drop(shown);
            on_click(click);
        }
    }
}

/// Keeps the tokens the service hands out until their notification's
/// action, and returns a click on a notification this process shows.
fn follow(
    signal: Signal,
    shown: &mut HashMap<u32, String>,
    tokens: &mut HashMap<u32, String>,
) -> Option<Click> {
    match signal {
        Signal::ActivationToken(id, token) => {
            if shown.contains_key(&id) {
                tokens.insert(id, token);
            }
            None
        }
        Signal::ActionInvoked(id, action) => {
            let activation_token = tokens.remove(&id);
            if action != DEFAULT_ACTION {
                return None;
            }
            Some(Click {
                tag: shown.get(&id)?.clone(),
                activation_token,
            })
        }
        Signal::Closed(id) => {
            shown.remove(&id);
            tokens.remove(&id);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_on_a_shown_notification_opens_its_tag_with_its_token() {
        let mut shown = HashMap::from([(7, "v1:task:home:T:e1".to_string())]);
        let mut tokens = HashMap::new();
        let token = Signal::ActivationToken(7, "token-7".into());
        assert_eq!(follow(token, &mut shown, &mut tokens), None);
        let click = follow(
            Signal::ActionInvoked(7, DEFAULT_ACTION.into()),
            &mut shown,
            &mut tokens,
        );
        assert_eq!(
            click,
            Some(Click {
                tag: "v1:task:home:T:e1".into(),
                activation_token: Some("token-7".into()),
            })
        );
        assert!(tokens.is_empty(), "a token is used once");
    }

    #[test]
    fn other_notifications_actions_and_closed_ones_open_nothing() {
        let mut shown = HashMap::from([(7, "v1:task:home:T:e1".to_string())]);
        let mut tokens = HashMap::new();
        let other = Signal::ActionInvoked(8, DEFAULT_ACTION.into());
        assert_eq!(follow(other, &mut shown, &mut tokens), None);
        let button = Signal::ActionInvoked(7, "snooze".into());
        assert_eq!(follow(button, &mut shown, &mut tokens), None);
        assert_eq!(follow(Signal::Closed(7), &mut shown, &mut tokens), None);
        let late = Signal::ActionInvoked(7, DEFAULT_ACTION.into());
        assert_eq!(follow(late, &mut shown, &mut tokens), None);
    }

    #[test]
    fn a_click_without_a_token_still_opens() {
        let mut shown = HashMap::from([(7, "v1:digest:home:2026-11-08".to_string())]);
        let click = follow(
            Signal::ActionInvoked(7, DEFAULT_ACTION.into()),
            &mut shown,
            &mut HashMap::new(),
        );
        assert_eq!(click.map(|click| click.activation_token), Some(None));
    }
}

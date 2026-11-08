//! The quick-capture launcher: a panel over whatever app is in front, which
//! sends text, labels and images to the agent or straight into Aven. Hiding it
//! only takes its window off screen; what was typed waits for the next time it
//! opens.
use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

use accesskit::Role;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Entity, ExternalPaths, Global,
    ImageFormat, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels,
    SharedString, Subscription, SystemNotification, Transformation, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, img,
    point, prelude::*, radians, relative, size, svg,
};

use crate::actions::*;
use crate::adapter::{LauncherPrefs, PendingImage};
use crate::app::with_root;
use crate::draft::{CaretWord, Suggestion, Token, caret_word};
use crate::hotkey::{self, HotKey, Shortcut};
use crate::images::{pasted_image, read_image};
use crate::input::{Editor, EditorEvent, Mode, Tint};
use crate::kit::*;
use crate::launch::{
    Catalog, Created, LaunchEvent, LaunchJob, LaunchMode, Place, agent_text, launcher_suggestions,
    merged, plain_task, read, tokens,
};
use crate::platform;
use crate::theme::{Theme, theme};
use crate::tooltip::{Hint, Hinted};

const WIDTH: f32 = 660.;
/// Room around the panel for its shadow, which the window would cut off.
const MARGIN: f32 = 64.;
/// Tall enough for the longest text, images and a menu under the footer.
const WINDOW_HEIGHT: f32 = 640.;
const LINE_HEIGHT: f32 = 26.;
const MAX_TEXT_HEIGHT: f32 = 156.;
const MENU_MIN_WIDTH: f32 = 150.;
const SUGGEST_WIDTH: f32 = 230.;
const FOOTER_HEIGHT: f32 = 38.;
const RADIUS: f32 = 14.;
const TICK: Duration = Duration::from_millis(100);
/// How often the launcher looks at Option while it shows.
const MODIFIER_POLL: Duration = Duration::from_millis(50);
const SWEEP: Duration = Duration::from_millis(1100);
const SPIN: Duration = Duration::from_millis(700);
const PLACEHOLDER: &str = "Capture a task\u{2026}  #label  @project  !high";

/// What the launcher holds between showings. No project is Auto.
#[derive(Clone, Default)]
struct Payload {
    text: String,
    workspace: Option<String>,
    project: Option<String>,
    labels: Vec<String>,
    images: Vec<PendingImage>,
}

impl Payload {
    /// The payload after a submission: only where it went stays.
    fn sent(&self) -> Self {
        Self {
            workspace: self.workspace.clone(),
            project: self.project.clone(),
            ..Self::default()
        }
    }
}

#[derive(Default)]
struct Host {
    /// The launcher's window, which stays open once made and is only taken
    /// off screen, so it shows again at once.
    window: Option<WindowHandle<Launcher>>,
    /// The screen the window was placed on, which it leaves for a new one
    /// when the primary display changes.
    screen: Option<Bounds<Pixels>>,
    /// The payload the window starts with.
    kept: Payload,
    catalog: Catalog,
    /// Whether Return has the agent write the task when there is one.
    ai: bool,
    hot_key: Option<HotKey>,
    /// Notifications posted, so each gets its own tag.
    posted: u64,
}

impl Global for Host {}

/// Sets up the launcher and its shortcut. Says why the saved shortcut could
/// not be taken, if it could not.
pub fn init(prefs: &LauncherPrefs, cx: &mut App) -> Option<String> {
    cx.set_global(Host {
        kept: Payload {
            workspace: prefs.workspace.clone(),
            project: prefs.project.clone(),
            ..Payload::default()
        },
        ai: prefs.ai,
        ..Host::default()
    });
    if let Some(mut presses) = hotkey::listen() {
        cx.spawn(async move |cx| {
            while presses.recv().await.is_some() {
                cx.update(toggle);
            }
        })
        .detach();
    }
    let shortcut = prefs.shortcut.as_ref()?;
    set_shortcut(Some(shortcut), cx).err()
}

/// Takes `shortcut` for the launcher, freeing the one it had. None frees it.
pub fn set_shortcut(shortcut: Option<&Shortcut>, cx: &mut App) -> Result<(), String> {
    let host = cx.global_mut::<Host>();
    host.hot_key = None;
    if let Some(shortcut) = shortcut {
        host.hot_key = Some(hotkey::register(shortcut)?);
    }
    Ok(())
}

/// Sets whether Return has the agent write the task.
pub fn set_ai(ai: bool, cx: &mut App) {
    cx.global_mut::<Host>().ai = ai;
    update_open(cx, |_, _, cx| cx.notify());
}

/// Shows the launcher, or hides it when it is already in front.
pub fn toggle(cx: &mut App) {
    if let Some(window) = cx.global::<Host>().window {
        let hidden = window.update(cx, |launcher, window, cx| {
            if window.is_window_active() {
                launcher.hide(window, cx);
                true
            } else {
                false
            }
        });
        if hidden.unwrap_or(false) {
            return;
        }
    }
    show(cx);
}

pub fn show(cx: &mut App) {
    with_root(cx, |root, _, _| root.session.load_launcher());
    let Some(display) = cx.primary_display() else {
        return;
    };
    let screen = display.bounds();
    let host = cx.global::<Host>();
    if let Some(window) = host.window {
        let moved = host.screen != Some(screen);
        let shown = window.update(cx, |launcher, window, cx| {
            if moved {
                let payload = launcher.payload(cx);
                cx.global_mut::<Host>().kept = payload;
                window.remove_window();
            } else {
                launcher.appear(window, cx);
            }
        });
        if shown.is_ok() && !moved {
            return;
        }
        cx.global_mut::<Host>().window = None;
    }
    let width = px(WIDTH + MARGIN * 2.);
    let origin = point(
        screen.origin.x + (screen.size.width - width) / 2.,
        screen.origin.y + screen.size.height * 0.21 - px(MARGIN),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            origin,
            size(width, px(WINDOW_HEIGHT)),
        ))),
        titlebar: None,
        // `activate_window` shows it once it is set up, without a fade.
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some(crate::app::APP_ID.into()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        platform::plain_panel(window);
        let payload = std::mem::take(&mut cx.global_mut::<Host>().kept);
        cx.new(|cx| Launcher::new(payload, window, cx))
    });
    match opened {
        Ok(window) => {
            let host = cx.global_mut::<Host>();
            host.window = Some(window);
            host.screen = Some(screen);
            let _ = window.update(cx, |launcher, window, cx| launcher.appear(window, cx));
        }
        Err(error) => tracing::error!(%error, "the launcher could not open"),
    }
}

/// Lets go of the text, labels and images, as a finished handoff does.
pub fn clear(cx: &mut App) {
    if let Some(window) = cx.global::<Host>().window {
        let _ = window.update(cx, |launcher, _, cx| launcher.clear(cx));
    }
    let host = cx.global_mut::<Host>();
    host.kept = host.kept.sent();
}

pub fn set_catalog(catalog: Catalog, cx: &mut App) {
    cx.global_mut::<Host>().catalog = catalog;
    if let Some(window) = cx.global::<Host>().window {
        let _ = window.update(cx, |launcher, _, cx| launcher.retint(cx));
    }
}

/// The launcher's view, once its window is made.
fn update_open<R>(
    cx: &mut App,
    f: impl FnOnce(&mut Launcher, &mut Window, &mut Context<Launcher>) -> R,
) -> Option<R> {
    let window = cx.global::<Host>().window?;
    window.update(cx, f).ok()
}

/// Takes what a submission came to.
pub fn receive(event: LaunchEvent, cx: &mut App) {
    match event {
        LaunchEvent::Accepted(id) => {
            update_open(cx, |launcher, window, cx| {
                if launcher.phase == Phase::Sending(id) {
                    launcher.phase = Phase::Idle;
                    launcher.clear(cx);
                    launcher.hide(window, cx);
                }
            });
        }
        LaunchEvent::Refused(id, reason) => {
            let unseen = update_open(cx, |launcher, window, cx| {
                let waiting = launcher.phase == Phase::Sending(id);
                if waiting {
                    let (ai, _) = launcher.ai(cx);
                    let key = if ai { "\u{2325}\u{21b5}" } else { "\u{21b5}" };
                    launcher.fail(format!("{reason} {key} adds it without AI."), cx);
                }
                waiting && !window.is_window_active()
            });
            if unseen.unwrap_or(false) {
                notify_failure("Couldn\u{2019}t send task", &reason, cx);
            }
        }
        LaunchEvent::Created(created) => notify_created(&created, cx),
        LaunchEvent::Failed { title, body } => notify_failure(&title, &body, cx),
        LaunchEvent::Draft(id, workspace, task) => {
            let current = update_open(cx, |launcher, window, cx| {
                let current = launcher.drafting() == Some(id);
                if current {
                    launcher.settle(cx);
                    launcher.hide(window, cx);
                }
                current
            });
            if current.unwrap_or(false) {
                with_root(cx, |root, window, cx| {
                    root.hand_off_draft(workspace, *task, window, cx);
                });
            }
        }
        LaunchEvent::DraftFailed(id, reason) => {
            update_open(cx, |launcher, _, cx| {
                if launcher.drafting() == Some(id) {
                    launcher.fail(reason, cx);
                }
            });
        }
    }
}

/// The tag of a created task's notification, which a click hands back. A
/// later one for the same task replaces it.
fn created_tag(workspace: &str, task: &str) -> String {
    aven_host::notify::task_tag(workspace, task, "created")
}

/// What a created task's notification says under its title.
fn created_body(created: &Created) -> String {
    let mut place = format!("{} \u{b7} {}", created.reference, created.project);
    for label in &created.labels {
        place.push_str(&format!(" #{label}"));
    }
    format!("{}\n{place}", created.title)
}

/// Posts that a task was saved, and shows it in the main window too, since
/// macOS may hold notifications back.
fn notify_created(created: &Created, cx: &mut App) {
    let title = if created.plain {
        "Task added"
    } else {
        "Task created"
    };
    cx.show_system_notification(SystemNotification {
        tag: created_tag(&created.workspace, created.task.as_ref()).into(),
        title: title.into(),
        body: created_body(created).into(),
        actions: Vec::new(),
    });
    let created = created.clone();
    with_root(cx, move |root, _, cx| root.launched(title, &created, cx));
}

fn notify_failure(title: &str, body: &str, cx: &mut App) {
    let host = cx.global_mut::<Host>();
    host.posted += 1;
    let tag = format!("launch-failed:{}", host.posted);
    cx.show_system_notification(SystemNotification {
        tag: tag.into(),
        title: title.to_string().into(),
        body: body.to_string().into(),
        actions: Vec::new(),
    });
    let (title, body) = (title.to_string(), body.to_string());
    with_root(cx, move |root, _, cx| {
        root.session
            .show_toast(crate::toast::Toast::error(title).detail(body));
        cx.notify();
    });
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    /// A job sent and not yet taken or refused.
    Sending(u64),
    Drafting {
        id: u64,
        started: Instant,
    },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Workspace,
    Project,
    Labels,
}

impl MenuKind {
    fn anchor(self) -> &'static str {
        match self {
            Self::Workspace => "l-workspace",
            Self::Project => "l-project",
            Self::Labels => "l-labels",
        }
    }
}

struct Menu {
    kind: MenuKind,
    index: usize,
}

/// One row of a launcher menu.
struct Row {
    label: String,
    mark: Mark,
    checked: bool,
}

enum Mark {
    Sparkle,
    Project(String),
    Label,
}

/// What a submission asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Send {
    Create,
    Review,
    Plain,
}

/// A submit key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Return,
    /// Option-Return.
    Other,
    /// Command-Return.
    Review,
}

/// What `key` sends. Return follows the AI setting, Option-Return does the
/// other, and without an agent everything is added as typed.
fn send_for(key: Key, ai: bool, agent: bool) -> Option<Send> {
    let ai = ai && agent;
    match key {
        Key::Return if ai => Some(Send::Create),
        Key::Other if !ai && agent => Some(Send::Create),
        Key::Return | Key::Other => Some(Send::Plain),
        Key::Review => agent.then_some(Send::Review),
    }
}

/// A thumbnail for image bytes GPUI can draw, by their magic numbers.
fn preview(bytes: &[u8]) -> Option<Arc<gpui::Image>> {
    let format = if bytes.starts_with(b"\x89PNG") {
        ImageFormat::Png
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        ImageFormat::Jpeg
    } else if bytes.starts_with(b"GIF8") {
        ImageFormat::Gif
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else {
        return None;
    };
    Some(Arc::new(gpui::Image::from_bytes(format, bytes.to_vec())))
}

/// The text that stands for `token` in launcher text.
fn token_text(token: &Token) -> String {
    match token {
        Token::Label(label) => format!("#{label}"),
        Token::Priority(priority) => format!("!{}", priority_name(*priority).to_lowercase()),
        Token::Project(key) => format!("@{key}"),
    }
}

pub struct Launcher {
    editor: Entity<Editor>,
    workspace: Option<String>,
    project: Option<String>,
    labels: Vec<String>,
    images: Vec<(PendingImage, Option<Arc<gpui::Image>>)>,
    phase: Phase,
    menu: Option<Menu>,
    /// The highlighted suggestion.
    suggestion: usize,
    /// The word whose suggestions Escape closed.
    dismissed: Option<CaretWord>,
    /// Whether Option alone is held, which swaps the key hints.
    alt: bool,
    /// Whether the window has been in front since it was shown, so losing
    /// that hides it.
    shown: bool,
    /// Whether the window is on screen.
    visible: bool,
    anchors: Anchors,
    _subscriptions: Vec<Subscription>,
}

/// The placeholder's prompt reads first, and the token hints recede.
fn placeholder_tints(t: &Theme) -> Vec<Tint> {
    let tokens = PLACEHOLDER.find('#').unwrap_or(PLACEHOLDER.len());
    let tint = |range, color| Tint {
        range,
        color,
        background: gpui::transparent_black(),
    };
    vec![
        tint(0..tokens, t.muted),
        tint(tokens..PLACEHOLDER.len(), t.faint.opacity(0.55)),
    ]
}

impl Launcher {
    fn new(payload: Payload, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::new(Mode::Multi, PLACEHOLDER, window, cx)
                .takes_images()
                .max_height(px(MAX_TEXT_HEIGHT));
            editor.set_text(&payload.text, cx);
            editor.set_placeholder_tints(placeholder_tints(&theme(cx)), cx);
            editor
        });
        let subscriptions = vec![
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.shown = true;
                } else if this.shown {
                    this.hide(window, cx);
                }
            }),
        ];
        let this = Self {
            editor,
            workspace: payload.workspace,
            project: payload.project,
            labels: payload.labels,
            images: payload
                .images
                .into_iter()
                .map(|image| {
                    let shown = preview(&image.bytes);
                    (image, shown)
                })
                .collect(),
            phase: Phase::Idle,
            menu: None,
            suggestion: 0,
            dismissed: None,
            alt: false,
            shown: false,
            visible: false,
            anchors: Anchors::default(),
            _subscriptions: subscriptions,
        };
        this.retint(cx);
        this
    }

    /// Brings the window in front with the text focused.
    fn appear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.activate_window();
        self.focus(window, cx);
        if !self.visible {
            self.visible = true;
            self.watch_option(cx);
        }
    }

    /// Follows Option while the window shows. A panel over another app gets
    /// no modifier events, so this reads the keyboard's state instead.
    fn watch_option(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(MODIFIER_POLL).await;
                let going = this.update(cx, |this, cx| {
                    let alt = platform::option_alone();
                    if this.visible && this.alt != alt {
                        this.alt = alt;
                        cx.notify();
                    }
                    this.visible
                });
                if !going.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.editor.read(cx).focus.clone();
        window.focus(&focus, cx);
    }

    /// The workspace the launcher sends to: the one picked while it still
    /// exists, or the main window's.
    fn place(&self, cx: &App) -> Place {
        let catalog = &cx.global::<Host>().catalog;
        self.workspace
            .as_deref()
            .and_then(|key| catalog.place(key))
            .or_else(|| catalog.place(&catalog.active))
            .or_else(|| catalog.places.first())
            .cloned()
            .unwrap_or_default()
    }

    fn payload(&self, cx: &App) -> Payload {
        Payload {
            text: self.editor.read(cx).text().to_string(),
            workspace: self.workspace.clone(),
            project: self.project.clone(),
            labels: self.labels.clone(),
            images: self.images.iter().map(|(image, _)| image.clone()).collect(),
        }
    }

    fn drafting(&self) -> Option<u64> {
        match self.phase {
            Phase::Drafting { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Ends drafting, whatever it came to, so the text can be edited again.
    fn settle(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Idle;
        self.editor
            .update(cx, |editor, cx| editor.set_read_only(false, cx));
        cx.notify();
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        self.settle(cx);
        self.phase = Phase::Error(reason);
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| editor.set_text("", cx));
        self.labels.clear();
        self.images.clear();
        self.menu = None;
        if matches!(self.phase, Phase::Error(_)) {
            self.phase = Phase::Idle;
        }
        self.retint(cx);
    }

    /// Takes the window off screen and keeps what it held. A draft under way is
    /// dropped, so its answer never arrives.
    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.drafting() {
            cx.defer(move |cx| with_root(cx, move |root, _, _| root.session.cancel_launch(id)));
            self.settle(cx);
        }
        self.shown = false;
        self.visible = false;
        self.alt = false;
        self.menu = None;
        cx.notify();
        platform::order_out(window);
    }

    /// Colours the text's tokens as what they set.
    fn retint(&self, cx: &mut Context<Self>) {
        let t = theme(cx);
        let place = self.place(cx);
        let tints = tokens(self.editor.read(cx).text(), &place)
            .into_iter()
            .map(|(range, token)| {
                let (color, background) = match token {
                    Token::Label(_) => (t.accent, t.tint),
                    Token::Priority(priority) => (priority_color(priority, &t), t.raised),
                    Token::Project(key) => (t.project(&key), t.project(&key).opacity(0.16)),
                };
                Tint {
                    range,
                    color,
                    background,
                }
            })
            .collect();
        self.editor
            .update(cx, |editor, cx| editor.set_tints(tints, cx));
        cx.notify();
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Changed => {
                if matches!(self.phase, Phase::Error(_)) {
                    self.phase = Phase::Idle;
                }
                self.suggestion = 0;
                self.menu = None;
                self.retint(cx);
            }
            EditorEvent::Submit => self.press(Key::Return, cx),
            // Escape lets go of what was typed; leaving the launcher any other
            // way keeps it for next time.
            EditorEvent::Cancel => {
                if self.menu.take().is_some() {
                    cx.notify();
                } else {
                    self.hide(window, cx);
                    self.clear(cx);
                }
            }
            EditorEvent::PastedImage(pasted) => {
                if let Some(image) = pasted_image(pasted.clone()) {
                    self.add_image(image, cx);
                }
            }
            _ => {}
        }
    }

    fn add_image(&mut self, image: PendingImage, cx: &mut Context<Self>) {
        if self.drafting().is_some()
            || self
                .images
                .iter()
                .any(|(held, _)| held.bytes == image.bytes)
        {
            return;
        }
        let shown = preview(&image.bytes);
        self.images.push((image, shown));
        if matches!(self.phase, Phase::Error(_)) {
            self.phase = Phase::Idle;
        }
        cx.notify();
    }

    /// Sends what the launcher holds as `how`, fixed as it stands now.
    fn send(&mut self, how: Send, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Sending(_) | Phase::Drafting { .. }) {
            return;
        }
        let place = self.place(cx);
        if place.key.is_empty() {
            return;
        }
        let text = self.editor.read(cx).text().to_string();
        if text.trim().is_empty() && self.images.is_empty() {
            return;
        }
        let mode = match how {
            Send::Create => LaunchMode::Create,
            Send::Review => LaunchMode::Review,
            Send::Plain => match plain_task(&text, &place, !self.images.is_empty()) {
                Some(task) => LaunchMode::Plain(task),
                None => return,
            },
        };
        let reading = read(&text, &place);
        let picked = self
            .project
            .clone()
            .filter(|key| place.project(key).is_some());
        let id = crate::launch::next_job_id();
        let job = LaunchJob {
            id,
            mode,
            workspace: place.key.clone(),
            project: reading.project.or(picked),
            labels: merged(&self.labels, &reading.labels),
            text: agent_text(&text, &place),
            images: self.images.iter().map(|(image, _)| image.clone()).collect(),
        };
        self.menu = None;
        if how == Send::Review {
            self.phase = Phase::Drafting {
                id,
                started: Instant::now(),
            };
            self.editor
                .update(cx, |editor, cx| editor.set_read_only(true, cx));
            self.tick(id, cx);
        } else {
            self.phase = Phase::Sending(id);
        }
        cx.defer(move |cx| with_root(cx, |root, _, _| root.session.launch(job)));
        cx.notify();
    }

    /// Redraws the elapsed time while draft `id` is under way.
    fn tick(&self, id: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let going = this.update(cx, |this, cx| {
                    cx.notify();
                    this.drafting() == Some(id)
                });
                if !going.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    fn launch_review(&mut self, _: &LaunchReview, _: &mut Window, cx: &mut Context<Self>) {
        self.press(Key::Review, cx);
    }

    fn launch_other(&mut self, _: &LaunchOther, _: &mut Window, cx: &mut Context<Self>) {
        self.press(Key::Other, cx);
    }

    fn press(&mut self, key: Key, cx: &mut Context<Self>) {
        let (ai, agent) = self.ai(cx);
        if let Some(how) = send_for(key, ai, agent) {
            self.send(how, cx);
        }
    }

    /// The AI setting and whether there is an agent to follow it.
    fn ai(&self, cx: &App) -> (bool, bool) {
        let host = cx.global::<Host>();
        (host.ai, host.catalog.agent)
    }

    fn pick_workspace(&mut self, key: String, cx: &mut Context<Self>) {
        if self.place(cx).key != key {
            self.project = None;
            self.labels.clear();
        }
        self.workspace = Some(key);
        self.remember(cx);
        self.retint(cx);
    }

    fn pick_project(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        self.project = key;
        self.remember(cx);
        cx.notify();
    }

    fn toggle_label(&mut self, label: String, cx: &mut Context<Self>) {
        if let Some(at) = self.labels.iter().position(|held| *held == label) {
            self.labels.remove(at);
        } else {
            self.labels.push(label);
        }
        cx.notify();
    }

    /// Saves where the launcher sends, for the next time Aven runs.
    fn remember(&self, cx: &mut Context<Self>) {
        let (workspace, project) = (self.workspace.clone(), self.project.clone());
        cx.defer(move |cx| {
            with_root(cx, move |root, _, _| {
                root.prefs.launcher.workspace = workspace;
                root.prefs.launcher.project = project;
                root.save_prefs();
            });
        });
    }

    fn rows(&self, kind: MenuKind, cx: &App) -> Vec<(Row, MenuPick)> {
        let place = self.place(cx);
        match kind {
            MenuKind::Workspace => cx
                .global::<Host>()
                .catalog
                .places
                .iter()
                .map(|other| {
                    let row = Row {
                        label: other.name.clone(),
                        mark: Mark::Label,
                        checked: other.key == place.key,
                    };
                    (row, MenuPick::Workspace(other.key.clone()))
                })
                .collect(),
            MenuKind::Project => {
                let picked = self
                    .project
                    .as_deref()
                    .filter(|key| place.project(key).is_some());
                let auto = Row {
                    label: "Auto".to_string(),
                    mark: Mark::Sparkle,
                    checked: picked.is_none(),
                };
                std::iter::once((auto, MenuPick::Project(None)))
                    .chain(place.projects.iter().map(|project| {
                        let row = Row {
                            label: project.name.clone(),
                            mark: Mark::Project(project.key.clone()),
                            checked: picked == Some(project.key.as_str()),
                        };
                        (row, MenuPick::Project(Some(project.key.clone())))
                    }))
                    .collect()
            }
            MenuKind::Labels => place
                .labels
                .iter()
                .map(|label| {
                    let row = Row {
                        label: label.clone(),
                        mark: Mark::Label,
                        checked: self.labels.contains(label),
                    };
                    (row, MenuPick::Label(label.clone()))
                })
                .collect(),
        }
    }

    fn open_menu(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.as_ref().is_some_and(|menu| menu.kind == kind) {
            self.menu = None;
        } else {
            let index = self
                .rows(kind, cx)
                .iter()
                .position(|(row, _)| row.checked && kind != MenuKind::Labels)
                .unwrap_or(0);
            self.menu = Some(Menu { kind, index });
        }
        self.focus(window, cx);
        cx.notify();
    }

    fn take_pick(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(kind) = self.menu.as_ref().map(|menu| menu.kind) else {
            return;
        };
        let Some((_, pick)) = self.rows(kind, cx).into_iter().nth(at) else {
            return;
        };
        match pick {
            MenuPick::Workspace(key) => {
                self.menu = None;
                self.pick_workspace(key, cx);
            }
            MenuPick::Project(key) => {
                self.menu = None;
                self.pick_project(key, cx);
            }
            MenuPick::Label(label) => {
                if let Some(menu) = self.menu.as_mut() {
                    menu.index = at;
                }
                self.toggle_label(label, cx);
            }
        }
    }

    /// What the word at the caret could become, while it is being typed.
    fn suggestions(&self, window: &Window, cx: &App) -> Option<(CaretWord, Vec<Suggestion>)> {
        if self.menu.is_some() || self.drafting().is_some() {
            return None;
        }
        let editor = self.editor.read(cx);
        if !editor.focus.is_focused(window) || editor.has_selection() || editor.is_composing() {
            return None;
        }
        let word = caret_word(editor.text(), editor.cursor())?;
        if self.dismissed.as_ref() == Some(&word) {
            return None;
        }
        let place = self.place(cx);
        let mut held = self.labels.clone();
        for (range, token) in tokens(editor.text(), &place) {
            if let Token::Label(label) = token
                && range.start != word.start
                && !held.contains(&label)
            {
                held.push(label);
            }
        }
        let found = launcher_suggestions(&word, &held, &place);
        (!found.is_empty()).then_some((word, found))
    }

    /// Writes suggestion `at` over the word under the caret, and a space.
    fn take_suggestion(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((word, mut found)) = self.suggestions(window, cx) else {
            return;
        };
        let editor = self.editor.read(cx);
        let text = editor.text();
        let rest = &text[editor.cursor()..];
        let end = editor.cursor() + rest.find(char::is_whitespace).unwrap_or(rest.len());
        let spaced = text[end..].starts_with(char::is_whitespace);
        let suggestion = found.swap_remove(at.min(found.len() - 1));
        let mut written = token_text(&suggestion.token);
        if !spaced {
            written.push(' ');
        }
        self.editor.update(cx, |editor, cx| {
            editor.replace(word.start..end, &written, window, cx);
        });
        self.dismissed = None;
        self.retint(cx);
    }

    fn suggest_next(&mut self, _: &SuggestNext, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    fn suggest_previous(
        &mut self,
        _: &SuggestPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step(-1, window, cx);
    }

    fn step(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = match self.menu.as_ref() {
            Some(menu) => self.rows(menu.kind, cx).len(),
            None => self
                .suggestions(window, cx)
                .map_or(0, |(_, found)| found.len()),
        };
        if count == 0 {
            return;
        }
        let at = match self.menu.as_mut() {
            Some(menu) => &mut menu.index,
            None => &mut self.suggestion,
        };
        *at = ((*at).min(count - 1) as isize + step).rem_euclid(count as isize) as usize;
        cx.notify();
    }

    fn accept_suggestion(
        &mut self,
        _: &AcceptSuggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.menu.as_ref() {
            Some(menu) => self.take_pick(menu.index, cx),
            None => self.take_suggestion(self.suggestion, window, cx),
        }
    }

    fn close_suggestions(
        &mut self,
        _: &CloseSuggestions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.take().is_none() {
            self.dismissed = self.suggestions(window, cx).map(|(word, _)| word);
        }
        cx.notify();
    }

    fn render_thumbs(&self, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement + use<> {
        let mut thumbs = div()
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .pl(px(50.))
            .pr(px(18.))
            .pb(px(14.));
        for (ix, (image, shown)) in self.images.iter().enumerate() {
            let face = match shown {
                Some(shown) => img(shown.clone())
                    .size_full()
                    .rounded(px(6.))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                None => small(Text::Caption)
                    .p(px(6.))
                    .text_color(t.muted)
                    .overflow_hidden()
                    .child(image.name.clone())
                    .into_any_element(),
            };
            let remove = div()
                .id(("l-image-remove", ix))
                .role(Role::Button)
                .aria_label(format!("Remove {}", image.name))
                .absolute()
                .top(px(-6.))
                .right(px(-6.))
                .size(px(18.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.raised)
                .text_color(t.muted)
                .text(Text::Secondary)
                .shadow(vec![t.inset_ring(t.line2)])
                .invisible()
                .group_hover("l-thumb", |style| style.visible())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.drafting().is_none() && ix < this.images.len() {
                        this.images.remove(ix);
                        cx.notify();
                    }
                }))
                .child("×");
            thumbs = thumbs.child(
                div()
                    .id(("l-image", ix))
                    .group("l-thumb")
                    .relative()
                    .flex_none()
                    .w(px(64.))
                    .h(px(44.))
                    .rounded(px(6.))
                    .bg(t.raised)
                    .shadow(vec![t.inset_ring(t.line2)])
                    .hint(Hint::new(image.name.clone()))
                    .child(face)
                    .child(remove),
            );
        }
        thumbs
    }

    fn pick_button(
        &self,
        kind: MenuKind,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self.menu.as_ref().is_some_and(|menu| menu.kind == kind);
        let (raised, ink) = (t.raised, t.ink);
        div()
            .id(kind.anchor())
            .role(Role::Button)
            .aria_label(label.into())
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .px(px(8.))
            .rounded(px(6.))
            .whitespace_nowrap()
            .when(open, |button| button.bg(raised).text_color(ink))
            .hover(move |style| style.bg(raised).text_color(ink))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| this.open_menu(kind, window, cx)))
            .child(anchor_probe(kind.anchor(), &self.anchors))
    }

    fn chip(t: &Theme) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(22.))
            .pl(px(8.))
            .pr(px(3.))
            .rounded_full()
            .text(Text::Secondary)
            .whitespace_nowrap()
            .bg(t.raised)
            .text_color(t.body)
            .shadow(vec![t.inset_ring(t.line2)])
    }

    /// A label and its key.
    fn hint(key: &'static str, what: &'static str, t: &Theme) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(what)
            .child(kbd(key, t.faint))
    }

    fn render_footer(&self, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        let foot = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .min_h(px(FOOTER_HEIGHT))
            .pl(px(10.))
            .pr(px(12.))
            .bg(t.panel)
            .rounded_b(px(RADIUS))
            .text(Text::Secondary)
            .text_color(t.muted);
        if self.drafting().is_some() {
            return foot
                .child(
                    div()
                        .pl(px(8.))
                        .text_color(t.body)
                        .child("Drafting with your agent \u{2014} opens in Aven to review"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(t.faint)
                        .child(Self::hint("esc", "Cancel", t)),
                )
                .into_any_element();
        }
        if let Phase::Error(reason) = &self.phase {
            return foot
                .py(px(8.))
                .child(
                    div()
                        .pl(px(6.))
                        .text_color(t.bad)
                        .child(format!("\u{26a0} {reason}")),
                )
                .into_any_element();
        }
        let place = self.place(cx);
        let reading = read(self.editor.read(cx).text(), &place);
        let (ai, agent) = self.ai(cx);
        let ai = ai && agent;
        // Option previews the other key, which only differs with an agent.
        let other = self.alt && agent;
        // Without the agent, Auto is the workspace's first project.
        let project = reading
            .project
            .as_deref()
            .or(self.project.as_deref())
            .and_then(|key| place.project(key))
            .or_else(|| place.projects.first().filter(|_| ai == other));
        let workspace = self
            .pick_button(MenuKind::Workspace, place.name.clone(), cx, t)
            .child(place.name.clone());
        let project = match project {
            Some(project) => self
                .pick_button(MenuKind::Project, project.name.clone(), cx, t)
                .child(project_dot(&project.key, t).mx(px(0.)))
                .child(project.name.clone()),
            None => self
                .pick_button(MenuKind::Project, "Auto", cx, t)
                .child(icon("icons/ic-sparkle.svg", t.muted).size(px(12.)))
                .child("Auto"),
        };
        let mut chips = div().flex().items_center().gap(px(5.)).overflow_hidden();
        for (ix, label) in self.labels.iter().enumerate() {
            let label = label.clone();
            chips = chips.child(
                Self::chip(t).child(label.clone()).child(
                    div()
                        .id(("l-label-remove", ix))
                        .role(Role::Button)
                        .aria_label(format!("Remove {label}"))
                        .size(px(16.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t.faint)
                        .hover(|style| style.bg(t.line2).text_color(t.ink))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_label(label.clone(), cx);
                        }))
                        .child("×"),
                ),
            );
        }
        for (ix, label) in reading.labels.iter().enumerate() {
            if self.labels.contains(label) {
                continue;
            }
            chips = chips.child(
                Self::chip(t)
                    .id(("l-label-typed", ix))
                    .pr(px(8.))
                    .bg(t.tint)
                    .text_color(t.accent)
                    .shadow(vec![t.inset_ring(t.tint_line)])
                    .hint(Hint::new(format!("From #{label} in the text")))
                    .child(label.clone()),
            );
        }
        // An empty row would add the footer's gap twice after the divider.
        let has_chips = !self.labels.is_empty() || !reading.labels.is_empty();
        let add = if has_chips { "+" } else { "+ Label" };
        let keys = div().flex().items_center().gap(px(14.)).whitespace_nowrap();
        let keys =
            match (other, ai) {
                (true, true) => keys.text_color(t.body).child(Self::hint(
                    by_platform!("⌥↩", "Alt+↩"),
                    "Add without AI",
                    t,
                )),
                (true, false) => keys.text_color(t.body).child(Self::hint(
                    by_platform!("⌥↩", "Alt+↩"),
                    "Create with AI",
                    t,
                )),
                (false, true) => keys
                    .text_color(t.faint)
                    .child(
                        Self::hint("↩", "Create with AI", t)
                            .id("l-create-hint")
                            .hint(Hint::new(by_platform!(
                                "Hold ⌥ to add without AI",
                                "Alt+↩ adds without AI"
                            ))),
                    )
                    .child(Self::hint(by_platform!("⌘↩", "Ctrl+↩"), "Review", t)),
                (false, false) => keys.text_color(t.faint).child(
                    Self::hint("↩", "Add", t)
                        .id("l-create-hint")
                        .when(agent, |hint| {
                            hint.hint(Hint::new(by_platform!(
                                "Hold ⌥ to create with AI",
                                "Alt+↩ creates with AI"
                            )))
                        }),
                ),
            };
        foot.child(workspace)
            // The sparkle's own margin sits right of the slash, so it leans left.
            .child(
                div()
                    .ml(px(-0.5))
                    .mr(px(-3.5))
                    .text_color(t.line2)
                    .child("/"),
            )
            .child(project)
            .child(
                div()
                    .flex_none()
                    .w(px(1.))
                    .h(px(14.))
                    .mx(px(6.))
                    .bg(t.line2),
            )
            .when(has_chips, |foot| foot.child(chips))
            .child(
                self.pick_button(MenuKind::Labels, "Add label", cx, t)
                    .child(add),
            )
            .child(div().flex_1())
            .child(keys)
            .into_any_element()
    }

    fn render_menu(&self, menu: &Menu, cx: &mut Context<Self>, t: &Theme) -> AnyElement {
        let anchor = self
            .anchors
            .borrow()
            .get(menu.kind.anchor())
            .copied()
            .unwrap_or_default();
        let title = match menu.kind {
            MenuKind::Workspace => Some("Workspace"),
            MenuKind::Project => Some("Project hint"),
            MenuKind::Labels => None,
        };
        let rows = self.rows(menu.kind, cx);
        let lead = menu.kind != MenuKind::Workspace;
        let mut list = div().flex().flex_col().p(px(5.));
        if let Some(title) = title {
            list = list.child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(6.))
                    .text(Text::Caption)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .child(title),
            );
        }
        if rows.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .min_h(px(28.))
                    .flex()
                    .items_center()
                    .text_color(t.faint)
                    .child("No labels"),
            );
        }
        for (ix, (row, _)) in rows.iter().enumerate() {
            let on = ix == menu.index;
            let mark = lead.then(|| match &row.mark {
                Mark::Sparkle => icon("icons/ic-sparkle.svg", t.muted)
                    .size(px(12.))
                    .mx(px(2.))
                    .into_any_element(),
                Mark::Project(key) => project_dot(key, t).mx(px(3.)).into_any_element(),
                Mark::Label => icon("icons/ic-labels.svg", t.muted)
                    .size(px(14.))
                    .mx(px(1.))
                    .into_any_element(),
            });
            list = list.child(
                div()
                    .id(("l-menu-item", ix))
                    .role(Role::MenuItem)
                    .aria_label(row.label.clone())
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .min_h(px(28.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .whitespace_nowrap()
                    .text_color(if on { t.ink } else { t.body })
                    .when(on, |row| row.bg(t.pick()))
                    .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                        if let Some(menu) = this.menu.as_mut().filter(|menu| menu.index != ix) {
                            menu.index = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.take_pick(ix, cx)))
                    .children(mark)
                    .child(div().flex_1().child(row.label.clone()))
                    .child(
                        div()
                            .flex_none()
                            .w(px(12.))
                            .ml(px(10.))
                            .text_color(t.accent)
                            .child(if row.checked { "✓" } else { "" }),
                    ),
            );
        }
        div()
            .id("l-menu")
            .role(Role::Menu)
            .absolute()
            .left(anchor.left())
            .top(anchor.bottom() + px(6.))
            .min_w(px(MENU_MIN_WIDTH))
            .rounded(px(10.))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .text(Text::Body)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list)
            .into_any_element()
    }

    fn render_suggestions(
        &self,
        word: &CaretWord,
        found: &[Suggestion],
        window: &Window,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> AnyElement {
        let picked = self.suggestion.min(found.len() - 1);
        let origin = self
            .editor
            .read(cx)
            .offset_origin(word.start)
            .unwrap_or_default();
        let viewport = window.viewport_size();
        let left = (origin.x - px(13.))
            .min(viewport.width - px(SUGGEST_WIDTH + 8.))
            .max(px(8.));
        let mut rows = div().p(px(5.)).flex().flex_col();
        for (ix, suggestion) in found.iter().enumerate() {
            let mark = match &suggestion.token {
                Token::Label(_) => icon("icons/ic-labels.svg", t.muted)
                    .size(px(14.))
                    .into_any_element(),
                Token::Priority(priority) => priority_icon(*priority, t).into_any_element(),
                Token::Project(key) => project_dot(key, t).into_any_element(),
            };
            rows = rows.child(
                div()
                    .id(("l-suggestion", ix))
                    .role(Role::ListBoxOption)
                    .aria_label(suggestion.label.clone())
                    .aria_selected(ix == picked)
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .min_h(px(28.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .text_color(if ix == picked { t.ink } else { t.body })
                    .when(ix == picked, |row| row.bg(t.pick()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.take_suggestion(ix, window, cx);
                        }),
                    )
                    .child(mark)
                    .child(div().min_w_0().truncate().child(suggestion.label.clone()))
                    .when(!suggestion.hint.is_empty(), |row| {
                        row.child(
                            small(Text::Secondary)
                                .ml_auto()
                                .flex_none()
                                .text_color(t.faint)
                                .child(suggestion.hint.clone()),
                        )
                    }),
            );
        }
        div()
            .id("l-suggestions")
            .role(Role::List)
            .absolute()
            .left(left)
            .top(origin.y + px(LINE_HEIGHT + 2.))
            .w(px(SUGGEST_WIDTH))
            .rounded(px(10.))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .text(Text::Body)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(rows)
            .into_any_element()
    }
}

/// What a launcher menu row picks.
enum MenuPick {
    Workspace(String),
    Project(Option<String>),
    Label(String),
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let logo = if t.dark {
            "logo-dark.svg"
        } else {
            "logo-light.svg"
        };
        let drafting = match self.phase {
            Phase::Drafting { started, .. } => Some(started.elapsed()),
            _ => None,
        };
        let suggesting = self.suggestions(window, cx);
        let status = drafting.map(|elapsed| {
            div()
                .mt(px(5.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .text(Text::Secondary)
                .text_color(t.faint)
                .child(
                    svg()
                        .path("icons/toast-progress.svg")
                        .size(px(13.))
                        .text_color(t.accent)
                        .with_animation("l-spin", Animation::new(SPIN).repeat(), |glyph, delta| {
                            glyph.with_transformation(Transformation::rotate(radians(delta * TAU)))
                        }),
                )
                .child(format!("{:.1}s", elapsed.as_secs_f32()))
        });
        let main = div()
            .flex()
            .items_start()
            .gap(px(12.))
            .pt(px(17.))
            .px(px(18.))
            .pb(px(15.))
            .child(gpui::img(logo).flex_none().size(px(20.)).mt(px(3.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text(Text::Heading)
                    .line_height(px(LINE_HEIGHT))
                    .text_color(if drafting.is_some() { t.muted } else { t.ink })
                    .when(suggesting.is_some() || self.menu.is_some(), |field| {
                        field.key_context("Suggesting")
                    })
                    .child(self.editor.clone()),
            )
            .children(status);
        let progress = div()
            .relative()
            .h(px(if drafting.is_some() { 2. } else { 1. }))
            .bg(t.line)
            .overflow_hidden()
            .when(drafting.is_some(), |bar| {
                bar.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(relative(0.34))
                        .bg(t.accent)
                        .with_animation(
                            "l-sweep",
                            Animation::new(SWEEP).repeat(),
                            |sweep, delta| sweep.left(relative(-0.34 + delta * 1.34)),
                        ),
                )
            });
        let ring = t.inset_ring(t.tint_line);
        let panel = div()
            .id("launcher")
            .role(Role::Dialog)
            .aria_label("Capture a task")
            .absolute()
            .top(px(MARGIN))
            .left(px(MARGIN))
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .rounded(px(RADIUS))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .overflow_hidden()
            .occlude()
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.shadow(vec![ring.clone()]))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                for image in paths.paths().iter().filter_map(|path| read_image(path)) {
                    this.add_image(image, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if this.menu.take().is_some() {
                        cx.notify();
                    }
                    this.focus(window, cx);
                }),
            )
            .child(main)
            .when(!self.images.is_empty(), |panel| {
                panel.child(self.render_thumbs(cx, &t))
            })
            .child(progress)
            .child(self.render_footer(cx, &t));
        let suggest =
            suggesting.map(|(word, found)| self.render_suggestions(&word, &found, window, cx, &t));
        let menu = self
            .menu
            .as_ref()
            .map(|menu| self.render_menu(menu, cx, &t));
        let layer = div()
            .id("launcher-layer")
            .key_context("Launcher")
            .size_full()
            .font_family(platform::ui_font())
            .on_action(cx.listener(Self::launch_review))
            .on_action(cx.listener(Self::launch_other))
            .on_action(cx.listener(Self::suggest_next))
            .on_action(cx.listener(Self::suggest_previous))
            .on_action(cx.listener(Self::accept_suggestion))
            .on_action(cx.listener(Self::close_suggestions))
            .on_key_down(cx.listener(|this, _: &KeyDownEvent, _, _| this.dismissed = None))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.hide(window, cx)),
            )
            .child(panel)
            .children(suggest)
            .children(menu);
        crate::tooltip::host(div().size_full().child(layer), window, cx, &t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aven_core::choices::TaskPriority;

    #[test]
    fn notification_tags_name_the_task_and_its_workspace() {
        let tag = created_tag("work", "01JABC");
        assert_eq!(
            aven_host::notify::target(&tag),
            Some(aven_host::notify::Target::Task {
                workspace: "work".to_string(),
                task: "01JABC".to_string(),
            })
        );
    }

    #[test]
    fn previews_only_formats_gpui_draws() {
        assert!(preview(b"\x89PNG\r\n\x1a\n....").is_some());
        assert!(preview(&[0xff, 0xd8, 0xff, 0xe0]).is_some());
        assert!(preview(b"RIFF\0\0\0\0WEBPVP8 ").is_some());
        assert!(preview(b"\0\0\0\x18ftypheic").is_none());
    }

    #[test]
    fn launch_keys_follow_the_ai_setting_and_option_swaps_them() {
        use Key::*;
        let sends = |ai, agent| [Return, Other, Review].map(|key| send_for(key, ai, agent));
        assert_eq!(
            sends(true, true),
            [Some(Send::Create), Some(Send::Plain), Some(Send::Review)]
        );
        assert_eq!(
            sends(false, true),
            [Some(Send::Plain), Some(Send::Create), Some(Send::Review)]
        );
        for ai in [true, false] {
            assert_eq!(
                sends(ai, false),
                [Some(Send::Plain), Some(Send::Plain), None]
            );
        }
    }

    #[test]
    fn the_placeholder_prompt_and_its_token_hints_tint_apart() {
        let tints = placeholder_tints(&Theme::dark());
        let ranges: Vec<_> = tints.iter().map(|tint| tint.range.clone()).collect();
        let hints = PLACEHOLDER.find('#').unwrap();
        assert_eq!(ranges, [0..hints, hints..PLACEHOLDER.len()]);
        assert!(PLACEHOLDER[..hints].starts_with("Capture a task"));
    }

    #[test]
    fn accepted_tokens_read_back_as_themselves() {
        assert_eq!(token_text(&Token::Label("bug".to_string())), "#bug");
        assert_eq!(token_text(&Token::Priority(TaskPriority::High)), "!high");
        assert_eq!(token_text(&Token::Project("home".to_string())), "@home");
    }

    #[test]
    fn a_sent_payload_keeps_only_its_destination() {
        let payload = Payload {
            text: "call".to_string(),
            workspace: Some("work".to_string()),
            project: Some("app".to_string()),
            labels: vec!["bug".to_string()],
            images: vec![PendingImage {
                name: "a.png".to_string(),
                bytes: Arc::new(vec![1]),
            }],
        };
        let sent = payload.sent();
        assert_eq!(sent.workspace.as_deref(), Some("work"));
        assert_eq!(sent.project.as_deref(), Some("app"));
        assert!(sent.text.is_empty() && sent.labels.is_empty() && sent.images.is_empty());
    }
}

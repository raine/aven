//! Surfaces above the window: the choice menu, the composer and the toast.
use std::time::Duration;

use accesskit::Role;
use aven_core::choices::{TaskPriority, TaskStatus};
use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Entity, FocusHandle, FontWeight,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Subscription, Window, div, prelude::*, px,
};

use crate::actions::*;
use crate::input::{Editor, EditorEvent, Mode};
use crate::kit::*;
use crate::shell::Root;
use crate::theme::theme;

pub const TOAST_DURATION: Duration = Duration::from_secs(5);
const MENU_ENTER: Duration = Duration::from_millis(140);
const DIALOG_CLOSE: Duration = Duration::from_millis(120);
const COMPOSER_WIDTH: f32 = 560.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Status(TaskStatus),
    Priority(TaskPriority),
}

fn choices(status: bool) -> Vec<(Choice, &'static str)> {
    if status {
        STATUSES
            .iter()
            .map(|(value, name)| (Choice::Status(*value), *name))
            .collect()
    } else {
        PRIORITIES
            .iter()
            .map(|(value, name)| (Choice::Priority(*value), *name))
            .collect()
    }
}

pub struct Menu {
    status: bool,
    input: Entity<Editor>,
    index: usize,
    anchor: Bounds<Pixels>,
    prior: Option<FocusHandle>,
    run: usize,
    _subscription: Subscription,
}

pub struct Composer {
    input: Entity<Editor>,
    prior: Option<FocusHandle>,
    run: usize,
    pub closing: bool,
    _subscription: Subscription,
}

impl Root {
    fn filtered(menu: &Menu, cx: &App) -> Vec<(Choice, &'static str)> {
        let query = menu.input.read(cx).text().to_lowercase();
        choices(menu.status)
            .into_iter()
            .filter(|(_, label)| {
                let label = label.to_lowercase();
                query.split_whitespace().all(|word| label.contains(word))
            })
            .collect()
    }

    fn open_menu(&mut self, status: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.selected.is_none() || self.menu.is_some() {
            return;
        }
        let key = if status { "status" } else { "priority" };
        let anchor = self
            .anchors
            .borrow()
            .get(key)
            .copied()
            .filter(|_| self.details_open);
        let anchor = anchor.unwrap_or_else(|| self.selected_row_bounds());
        let placeholder = if status { "Set status" } else { "Set priority" };
        let input = cx.new(|cx| Editor::new(Mode::Single, placeholder, window, cx).emit_arrows());
        let subscription = cx.subscribe_in(&input, window, Self::on_menu_event);
        let focus = input.read(cx).focus.clone();
        self.menu = Some(Menu {
            status,
            run: self.session.motion.next_run(),
            input,
            index: 0,
            anchor,
            prior: window.focused(cx),
            _subscription: subscription,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn change_status(
        &mut self,
        _: &ChangeStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(true, window, cx);
    }

    pub(crate) fn change_priority(
        &mut self,
        _: &ChangePriority,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu(false, window, cx);
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.take() {
            let focus = menu.prior.unwrap_or_else(|| self.focus.clone());
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let picked = Self::filtered(menu, cx)
            .get(index)
            .map(|(choice, _)| *choice);
        self.close_menu(window, cx);
        let (Some(id), Some(choice)) = (self.session.selected.clone(), picked) else {
            return;
        };
        match choice {
            Choice::Status(status) => {
                self.session.set_status(&id, status);
                self.after_status_motion(cx);
            }
            Choice::Priority(priority) => {
                self.session.set_priority(&id, priority);
                self.repaint_after(crate::session::PRIORITY_DURATION, cx);
            }
        }
        cx.notify();
    }

    fn on_menu_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let count = Self::filtered(menu, cx).len();
        let index = menu.index;
        match event {
            EditorEvent::Changed => self.set_menu_index(0, cx),
            EditorEvent::Up => self.set_menu_index(index.saturating_sub(1), cx),
            EditorEvent::Down => self.set_menu_index((index + 1).min(count.saturating_sub(1)), cx),
            EditorEvent::Submit => self.pick(index, window, cx),
            EditorEvent::Cancel => self.close_menu(window, cx),
            EditorEvent::Blur => {}
        }
    }

    fn set_menu_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut() {
            menu.index = index;
            cx.notify();
        }
    }

    pub(crate) fn render_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.menu.as_ref()?;
        let t = theme(cx);
        let entries = Self::filtered(menu, cx);
        let viewport = window.viewport_size();
        let height = (38. + 10. + 30. * entries.len().max(1) as f32).min(340.);
        let left = menu
            .anchor
            .left()
            .min(viewport.width - px(292.))
            .max(px(12.));
        let below = menu.anchor.bottom() + px(4.);
        let top = if below + px(height) > viewport.height - px(12.)
            && menu.anchor.top() - px(height + 4.) >= px(12.)
        {
            menu.anchor.top() - px(height + 4.)
        } else {
            below
        };
        let top = top.min(viewport.height - px(height + 12.)).max(px(12.));
        let index = menu.index.min(entries.len().saturating_sub(1));
        let mut rows = div().p(px(5.)).flex().flex_col();
        for (ix, (choice, label)) in entries.iter().enumerate() {
            let on = ix == index;
            rows = rows.child(
                div()
                    .id(("menu-item", ix))
                    .role(Role::MenuItem)
                    .aria_label(*label)
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .min_h(px(30.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .text_color(if on { t.ink } else { t.body })
                    .when(on, |row| row.bg(t.pick()))
                    .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                        if this.menu.as_ref().is_some_and(|menu| menu.index != ix) {
                            this.set_menu_index(ix, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(ix, window, cx)))
                    .child(match choice {
                        Choice::Status(status) => status_glyph(*status, &t).into_any_element(),
                        Choice::Priority(priority) => {
                            priority_icon(*priority, &t).into_any_element()
                        }
                    })
                    .child(div().truncate().child(*label)),
            );
        }
        if entries.is_empty() {
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .min_h(px(30.))
                    .px(px(8.))
                    .text_color(t.body)
                    .child("No results"),
            );
        }
        Some(
            div()
                .id("menu-layer")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| this.close_menu(window, cx)),
                )
                .child(
                    div()
                        .id("menu")
                        .role(Role::Menu)
                        .absolute()
                        .left(left)
                        .top(top)
                        .w(px(280.))
                        .max_h(px(340.))
                        .flex()
                        .flex_col()
                        .rounded(px(10.))
                        .bg(t.pop)
                        .shadow(t.menu_shadow())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex_none()
                                .h(px(38.))
                                .px(px(12.))
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(t.line)
                                .text_color(t.ink)
                                .child(menu.input.clone()),
                        )
                        .child(rows)
                        .with_animation(
                            ("menu-enter", menu.run),
                            Animation::new(MENU_ENTER),
                            move |menu, delta| {
                                let eased = 1. - (1. - delta).powi(3);
                                menu.top(top - px(4. * (1. - eased))).opacity(eased)
                            },
                        ),
                ),
        )
    }

    pub(crate) fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.open_composer(window, cx);
    }

    pub(crate) fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer_open() {
            return;
        }
        let input = cx.new(|cx| Editor::new(Mode::Single, "Task title", window, cx));
        let draft = std::mem::take(&mut self.composer_draft);
        input.update(cx, |input, cx| {
            input.set_text(&draft, cx);
            input.select_all_text(cx);
        });
        let subscription = cx.subscribe_in(&input, window, Self::on_composer_event);
        let focus = input.read(cx).focus.clone();
        self.composer = Some(Composer {
            run: self.session.motion.next_run(),
            prior: window.focused(cx),
            input,
            closing: false,
            _subscription: subscription,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Fades the composer out. Pointer input passes through while it does.
    fn close_composer(&mut self, keep_draft: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composer) = self.composer.as_mut().filter(|composer| !composer.closing) else {
            return;
        };
        composer.closing = true;
        let run = composer.run;
        let focus = composer.prior.clone().unwrap_or_else(|| self.focus.clone());
        self.composer_draft = if keep_draft {
            composer.input.read(cx).text().to_string()
        } else {
            String::new()
        };
        window.focus(&focus, cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DIALOG_CLOSE).await;
            this.update(cx, |this, cx| {
                let done = |composer: &Composer| composer.closing && composer.run == run;
                if this.composer.as_ref().is_some_and(done) {
                    this.composer = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composer) = self.composer.as_ref().filter(|composer| !composer.closing) else {
            return;
        };
        let title = composer.input.read(cx).text().trim().to_string();
        if title.is_empty() {
            return;
        }
        self.session.create(crate::adapter::NewTask::titled(title));
        self.close_composer(false, window, cx);
    }

    fn on_composer_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Submit => self.submit_composer(window, cx),
            EditorEvent::Cancel => self.close_composer(true, window, cx),
            _ => {}
        }
    }

    fn create_task(&mut self, _: &CreateTask, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_composer(window, cx);
    }

    pub(crate) fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        self.close_composer(true, window, cx);
    }

    pub(crate) fn render_composer(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let composer = self.composer.as_ref()?;
        let closing = composer.closing;
        let t = theme(cx);
        let viewport = window.viewport_size();
        let actions = div()
            .flex()
            .justify_end()
            .gap(px(4.))
            .px(px(10.))
            .py(px(8.))
            .border_t_1()
            .border_color(t.line)
            .child(
                btn("composer-discard", "Discard", &t)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_composer(false, window, cx)),
                    )
                    .child("Discard"),
            )
            .child(
                bare_btn("composer-create", "Create", &t)
                    .bg(t.fill)
                    .text_color(t.on_fill)
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|style| style.bg(t.fill_h))
                    .on_click(cx.listener(|this, _, window, cx| this.submit_composer(window, cx)))
                    .child("Create"),
            );
        let panel = div()
            .id("composer")
            .role(Role::Dialog)
            .aria_label("New task")
            .key_context("Dialog")
            .on_action(cx.listener(Self::create_task))
            .on_action(cx.listener(Self::dismiss))
            .absolute()
            .left((viewport.width - px(COMPOSER_WIDTH)) / 2.)
            .top(viewport.height * 0.11)
            .w(px(COMPOSER_WIDTH))
            .rounded(px(12.))
            .bg(t.pop)
            .shadow(t.dlg_shadow())
            .when(!closing, |panel| {
                panel
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .child(
                div()
                    .h(px(50.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .text_size(px(16.))
                    .text_color(t.ink)
                    .child(composer.input.clone()),
            )
            .child(actions);
        let layer = div()
            .id("composer-backdrop")
            .absolute()
            .inset_0()
            .bg(t.shade)
            .when(!closing, |layer| {
                layer.occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_composer(true, window, cx)
                    }),
                )
            })
            .child(panel);
        Some(if closing {
            layer
                .with_animation(
                    ("composer-close", composer.run),
                    Animation::new(DIALOG_CLOSE),
                    |layer, delta| layer.opacity(1. - delta),
                )
                .into_any_element()
        } else {
            layer.into_any_element()
        })
    }

    pub(crate) fn render_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let toast = self.session.toast.as_ref()?;
        let t = theme(cx);
        Some(
            div()
                .absolute()
                .bottom(px(24.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("toast")
                        .role(Role::Alert)
                        .aria_label(toast.message.clone())
                        .max_w(px(560.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(14.))
                        .py(px(9.))
                        .rounded(px(8.))
                        .bg(t.pop)
                        .shadow(t.menu_shadow())
                        .text_color(t.body)
                        .occlude()
                        .child(div().min_w_0().truncate().child(toast.message.clone()))
                        .when(toast.action.is_some(), |el| {
                            el.child(
                                div()
                                    .id("toast-action")
                                    .role(Role::Button)
                                    .aria_label("Retry")
                                    .flex_none()
                                    .text_color(t.accent)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.session.run_toast_action();
                                        cx.notify();
                                    }))
                                    .child("Retry"),
                            )
                        }),
                ),
        )
    }
}

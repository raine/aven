//! Surfaces above the window: menus and the toast.
use std::rc::Rc;
use std::time::Duration;

use accesskit::Role;
use aven_core::choices::{TaskPriority, TaskStatus};
use gpui::{
    Action, Animation, AnimationExt, App, Bounds, Context, Entity, FocusHandle, FontWeight,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollHandle, SharedString, Subscription,
    Window, div, prelude::*, relative,
};

use crate::input::{Editor, EditorEvent, Mode};
use crate::kit::*;
use crate::session::ToastAction;
use crate::shell::Root;
use crate::theme::theme;
use aven_core::labels::normalize_label;

pub const TOAST_DURATION: Duration = Duration::from_secs(5);
const MENU_ENTER: Duration = Duration::from_millis(140);
const MENU_WIDTH: f32 = 280.;
const PLAIN_MENU_WIDTH: f32 = 236.;
const WIDE_MENU_WIDTH: f32 = 620.;

pub type Run = Rc<dyn Fn(&mut Root, &mut Window, &mut Context<Root>)>;

#[derive(Clone)]
pub enum Mark {
    None,
    Status(TaskStatus),
    Priority(TaskPriority),
    Icon(&'static str),
    Project(String),
}

#[derive(Clone)]
pub struct Entry {
    pub head: bool,
    pub label: String,
    pub hint: String,
    pub mark: Mark,
    pub checked: bool,
    pub off: bool,
    pub sep: bool,
    pub danger: bool,
    /// Checking one entry of a group unchecks the rest.
    pub group: u8,
    run: Option<Run>,
}

impl Entry {
    pub fn new(
        label: impl Into<String>,
        run: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
    ) -> Self {
        Self {
            head: false,
            label: label.into(),
            hint: String::new(),
            mark: Mark::None,
            checked: false,
            off: false,
            sep: false,
            danger: false,
            group: 0,
            run: Some(Rc::new(run)),
        }
    }

    pub fn head(label: &str) -> Self {
        Self {
            head: true,
            run: None,
            ..Self::new(label, |_, _, _| {})
        }
    }

    /// An entry that dispatches a command, so it does what its key does.
    pub fn action(label: impl Into<String>, hint: &str, action: impl Action) -> Self {
        Self::new(label, move |_, window, cx| {
            window.dispatch_action(action.boxed_clone(), cx);
        })
        .hint(hint)
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn mark(mut self, mark: Mark) -> Self {
        self.mark = mark;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn off(mut self, off: bool) -> Self {
        self.off = off;
        self
    }

    pub fn sep(mut self) -> Self {
        self.sep = true;
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn group(mut self, group: u8) -> Self {
        self.group = group;
        self
    }
}

/// Makes the entry a create line stands for.
pub type Create = Rc<dyn Fn(String) -> Entry>;

#[derive(Default)]
pub struct MenuOptions {
    pub wide: bool,
    pub plain: bool,
    pub multi: bool,
    pub create: Option<Create>,
}

#[derive(Clone, Copy, PartialEq)]
enum Shown {
    Item(usize),
    Create,
}

pub struct Menu {
    entries: Vec<Entry>,
    options: MenuOptions,
    input: Entity<Editor>,
    index: usize,
    anchor: Bounds<Pixels>,
    prior: Option<FocusHandle>,
    run: usize,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

impl Menu {
    fn query(&self, cx: &App) -> String {
        self.input.read(cx).text().to_lowercase()
    }

    /// The entries the typed text leaves, in order.
    fn shown(&self, cx: &App) -> Vec<Shown> {
        let query = self.query(cx);
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut shown: Vec<Shown> = (0..self.entries.len())
            .filter(|ix| {
                let entry = &self.entries[*ix];
                if entry.head {
                    return words.is_empty();
                }
                let text = format!("{} {}", entry.label, entry.hint).to_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .map(Shown::Item)
            .collect();
        let exact = self
            .entries
            .iter()
            .any(|entry| entry.label.to_lowercase() == query.trim());
        if self.options.create.is_some() && !words.is_empty() && !exact {
            shown.push(Shown::Create);
        }
        shown
    }

    /// The shown lines that can be picked, in order.
    fn picks(&self, cx: &App) -> Vec<Shown> {
        let can_pick = |line: &Shown| match line {
            Shown::Item(ix) => !self.entries[*ix].head && !self.entries[*ix].off,
            Shown::Create => true,
        };
        self.shown(cx).into_iter().filter(can_pick).collect()
    }
}

impl Root {
    pub(crate) fn open_menu(
        &mut self,
        entries: Vec<Entry>,
        placeholder: &'static str,
        anchor: Bounds<Pixels>,
        options: MenuOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_some() {
            return;
        }
        let input = cx.new(|cx| Editor::new(Mode::Single, placeholder, window, cx).emit_arrows());
        let subscription = cx.subscribe_in(&input, window, Self::on_menu_event);
        let focus = input.read(cx).focus.clone();
        self.menu = Some(Menu {
            entries,
            index: if options.plain { usize::MAX } else { 0 },
            options,
            run: self.session.motion.next_run(),
            input,
            anchor,
            prior: window.focused(cx),
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.take() {
            let focus = menu.prior.unwrap_or_else(|| self.focus.clone());
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// The labels checked in the open menu, for a pick to act on.
    pub(crate) fn menu_checked(&self) -> Vec<String> {
        let entries = self.menu.iter().flat_map(|menu| &menu.entries);
        entries
            .filter(|entry| entry.checked)
            .map(|entry| entry.label.clone())
            .collect()
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let Some(line) = menu.picks(cx).get(index).copied() else {
            return;
        };
        let ix = match line {
            Shown::Item(ix) => ix,
            Shown::Create => {
                let label = normalize_label(&menu.query(cx));
                let create = menu.options.create.clone();
                let Some(entry) = create.map(|create| create(label)) else {
                    return;
                };
                menu.entries.push(entry);
                menu.entries.len() - 1
            }
        };
        let run = menu.entries[ix].run.clone();
        if menu.options.multi {
            let checked = !menu.entries[ix].checked;
            let group = menu.entries[ix].group;
            if group > 0 {
                for entry in &mut menu.entries {
                    if entry.group == group {
                        entry.checked = false;
                    }
                }
            }
            menu.entries[ix].checked = checked;
            let input = menu.input.clone();
            input.update(cx, |input, cx| input.set_text("", cx));
        } else {
            self.close_menu(window, cx);
        }
        if let Some(run) = run {
            run(self, window, cx);
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
        let count = menu.picks(cx).len();
        let index = if menu.index == usize::MAX {
            None
        } else {
            Some(menu.index)
        };
        match event {
            EditorEvent::Changed => self.set_menu_index(0, cx),
            EditorEvent::Up => {
                self.set_menu_index(index.map_or(0, |ix| ix.saturating_sub(1)), cx);
            }
            EditorEvent::Down => {
                let next = index.map_or(0, |ix| (ix + 1).min(count.saturating_sub(1)));
                self.set_menu_index(next, cx);
            }
            EditorEvent::Submit => self.pick(index.unwrap_or(0), window, cx),
            EditorEvent::Cancel => self.close_menu(window, cx),
            _ => {}
        }
    }

    fn set_menu_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut() {
            menu.index = index;
            let picks = menu.picks(cx);
            let line = picks.get(index).copied();
            let position = menu.shown(cx).iter().position(|shown| Some(*shown) == line);
            if let Some(position) = position {
                menu.scroll.scroll_to_item(position);
            }
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
        let (wide, plain, multi) = (menu.options.wide, menu.options.plain, menu.options.multi);
        let query = menu.query(cx);
        let typing = !query.trim().is_empty();
        let shown = menu.shown(cx);
        let picks = menu.picks(cx);
        let width = if wide {
            WIDE_MENU_WIDTH
        } else if plain {
            PLAIN_MENU_WIDTH
        } else {
            MENU_WIDTH
        };
        let viewport = window.viewport_size();
        let row = if plain { 28. } else { 30. };
        let input_height = match (wide, plain && !typing) {
            (true, _) => 48.,
            (_, true) => 0.,
            _ => 38.,
        };
        let body: f32 = shown.len().max(1) as f32 * row;
        let max_height = if wide {
            470.
        } else if plain {
            f32::MAX
        } else {
            340.
        };
        let height = (input_height + 10. + body).min(max_height);
        let (left, top) = if wide {
            ((viewport.width - px(width)) / 2., viewport.height * 0.13)
        } else {
            let left = menu
                .anchor
                .left()
                .min(viewport.width - px(width + 12.))
                .max(px(12.));
            let below = menu.anchor.bottom() + px(4.);
            let top = if below + px(height) > viewport.height - px(12.)
                && menu.anchor.top() - px(height + 4.) >= px(12.)
            {
                menu.anchor.top() - px(height + 4.)
            } else {
                below
            };
            (
                left,
                top.min(viewport.height - px(height + 12.)).max(px(12.)),
            )
        };
        let mut rows = div()
            .id("menu-rows")
            .flex_1()
            .min_h_0()
            .p(px(5.))
            .overflow_y_scroll()
            .track_scroll(&menu.scroll);
        for (ix, line) in shown.iter().enumerate() {
            let pick = picks.iter().position(|one| one == line);
            let on = pick.is_some() && pick == Some(menu.index);
            let created;
            let (label, entry): (&str, Option<&Entry>) = match line {
                Shown::Item(item) => {
                    let entry = &menu.entries[*item];
                    (entry.label.as_str(), Some(entry))
                }
                Shown::Create => {
                    created = format!("+ create \"{}\"", normalize_label(&query));
                    (created.as_str(), None)
                }
            };
            if entry.is_some_and(|entry| entry.head) {
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .min_h(px(26.))
                        .px(px(8.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.faint)
                        .child(label.to_string()),
                );
                continue;
            }
            let danger = entry.is_some_and(|entry| entry.danger);
            let off = entry.is_some_and(|entry| entry.off);
            if entry.is_some_and(|entry| entry.sep) && !typing {
                rows = rows.child(div().h(px(1.)).my(px(5.)).mx(px(-5.)).bg(t.line));
            }
            let mark = entry.map_or(Mark::None, |entry| entry.mark.clone());
            let color = if danger { t.bad } else { t.muted };
            let mark = match mark {
                Mark::None if plain => Some(div().flex_none().size(px(16.)).into_any_element()),
                Mark::None => None,
                Mark::Status(status) => Some(status_glyph(status, &t).into_any_element()),
                Mark::Priority(priority) => Some(priority_icon(priority, &t).into_any_element()),
                Mark::Icon(path) => Some(icon(path, color).into_any_element()),
                Mark::Project(key) => Some(project_dot(&key, &t).into_any_element()),
            };
            let hint = entry.map(|entry| entry.hint.clone()).unwrap_or_default();
            rows = rows.child(
                div()
                    .id(("menu-item", ix))
                    .role(Role::MenuItem)
                    .aria_label(label.to_string())
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .min_h(px(row))
                    .px(px(8.))
                    .rounded(px(6.))
                    .text_color(if danger {
                        t.bad
                    } else if off {
                        t.faint
                    } else if on {
                        t.ink
                    } else {
                        t.body
                    })
                    .when(on, |row| row.bg(t.pick()))
                    .when_some(pick, |row, pick| {
                        row.on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                            if this.menu.as_ref().is_some_and(|menu| menu.index != pick) {
                                if let Some(menu) = this.menu.as_mut() {
                                    menu.index = pick;
                                }
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.pick(pick, window, cx);
                            },
                        ))
                    })
                    .when(multi, |row| {
                        let checked = entry.is_some_and(|entry| entry.checked);
                        row.child(
                            div()
                                .flex_none()
                                .w(px(12.))
                                .text_color(t.accent)
                                .child(if checked { "✓" } else { "" }),
                        )
                    })
                    .children(mark)
                    .child(div().min_w_0().truncate().child(label.to_string()))
                    .when(!hint.is_empty(), |row| {
                        row.child(
                            small(12.)
                                .ml_auto()
                                .flex_none()
                                .text_color(t.faint)
                                .child(hint),
                        )
                    }),
            );
        }
        if shown.is_empty() {
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .min_h(px(row))
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
                .when(wide, |layer| layer.bg(t.shade))
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
                        .w(px(width))
                        .when(max_height < f32::MAX, |box_| box_.max_h(px(max_height)))
                        .flex()
                        .flex_col()
                        .rounded(px(10.))
                        .bg(t.pop)
                        .shadow(t.menu_shadow())
                        .overflow_hidden()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex_none()
                                .h(px(input_height))
                                .when(input_height > 0., |input| {
                                    input.border_b_1().border_color(t.line)
                                })
                                .overflow_hidden()
                                .px(px(if wide { 16. } else { 12. }))
                                .flex()
                                .items_center()
                                .text_color(t.ink)
                                .when(wide, |input| input.text_size(px(15.)))
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

    pub(crate) fn render_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let toast = self.session.toast.as_ref()?;
        let t = theme(cx);
        let toast_button = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .flex_none()
                .text_color(t.accent)
                .font_weight(FontWeight::SEMIBOLD)
                .child(label)
        };
        let action = toast.action.as_ref();
        let restore = matches!(action, Some(ToastAction::RestoreDraft));
        Some(
            div()
                .absolute()
                .bottom(px(22.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("toast")
                        .role(Role::Alert)
                        .aria_label(toast.message.clone())
                        .max_w(relative(0.7))
                        .min_h(px(34.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(12.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .bg(t.pop)
                        .shadow(t.menu_shadow())
                        .text_color(t.body)
                        .occlude()
                        .child(div().min_w_0().truncate().child(toast.message.clone()))
                        .when(toast.undo, |el| {
                            el.child(toast_button("toast-undo", "Undo".into()).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.session.run_toast_undo();
                                    cx.notify();
                                }),
                            ))
                        })
                        .when_some(action, |el, action| {
                            el.child(
                                toast_button("toast-action", action.label().into()).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        if restore {
                                            this.session.toast = None;
                                            this.restore_draft(window, cx);
                                        } else {
                                            this.session.run_toast_action();
                                        }
                                        cx.notify();
                                    }),
                                ),
                            )
                        }),
                ),
        )
    }
}

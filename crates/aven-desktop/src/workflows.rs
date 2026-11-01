//! Work on more than one field or task at a time: marks, epics, recurring
//! series, review and conflicts. Data rules stay in the host; this module only
//! decides what to ask and shows what came back.
use std::f32::consts::{PI, TAU};

use accesskit::Role;
use aven_core::choices::TaskStatus;
use aven_core::ids::TaskId;
use aven_core::query::RecurrenceSeriesLifecycleFilter as Lifecycle;
use aven_core::recurrence::{RecurrenceSeriesId, RecurrenceSeriesState};
use aven_host::store::ConflictTarget;
use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, Context, Div, Entity, FontWeight, Hsla,
    MouseButton, MouseDownEvent, PathBuilder, Pixels, SharedString, Stateful, Window, canvas, div,
    point, prelude::*, px,
};

use crate::actions::*;
use crate::adapter::{Bulk, Child, ChildGroup, Command, Detail, EpicProgress, SeriesRow, TaskRow};
use crate::dialogs::Dialog;
use crate::draft::Draft;
use crate::input::{Editor, EditorEvent, Mode};
use crate::kit::*;
use crate::markdown;
use crate::measure::measured;
use crate::overlays::{Entry, Mark, MenuOptions};
use crate::session::{ALL_CLEAR_DURATION, RING_DURATION, Review};
use crate::shell::Root;
use crate::theme::{Theme, theme};

pub(crate) const SERIES_ROW_HEIGHT: f32 = 34.;
pub(crate) const SERIES_HEAD_HEIGHT: f32 = 26.;
pub(crate) const LIFECYCLE_HEIGHT: f32 = 40.;
pub(crate) const ALL_CLEAR_HEIGHT: f32 = 96.;
const RING_SIZE: f32 = 16.;
const RING_RADIUS: f32 = 6.2;
const RING_WIDTH: f32 = 2.;
const LIFECYCLES: [(Lifecycle, &str); 5] = [
    (Lifecycle::ActiveOrPaused, "Active and paused"),
    (Lifecycle::Active, "Active"),
    (Lifecycle::Paused, "Paused"),
    (Lifecycle::Stopped, "Stopped"),
    (Lifecycle::All, "All"),
];

fn sub_head(heading: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .mt(px(20.))
        .mb(px(4.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.muted)
        .child(heading.into())
}

fn fact(label: &'static str, value: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .min_h(px(27.))
        .child(
            div()
                .flex_none()
                .w(px(84.))
                .text_color(t.faint)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.body)
                .child(value.into()),
        )
}

fn hint(text: impl Into<SharedString>, t: &Theme) -> Div {
    small(12.).mt(px(6.)).text_color(t.faint).child(text.into())
}

fn state_name(state: RecurrenceSeriesState) -> &'static str {
    state.as_str()
}

fn state_color(state: RecurrenceSeriesState, t: &Theme) -> Hsla {
    match state {
        RecurrenceSeriesState::Active => t.good,
        RecurrenceSeriesState::Paused => t.warn,
        RecurrenceSeriesState::Stopped => t.faint,
    }
}

/// What several tasks have in one field: their value, or how many there are.
fn mixed(rows: &[TaskRow], value: impl Fn(&TaskRow) -> String) -> String {
    let mut values: Vec<String> = rows.iter().map(value).collect();
    values.sort();
    values.dedup();
    match values.as_slice() {
        [one] => one.clone(),
        many => format!("{} different", many.len()),
    }
}

/// The epic's three parts as segments, `wide` filling its line.
pub(crate) fn epic_bar(progress: &EpicProgress, wide: bool, t: &Theme) -> Div {
    let part = |count: usize, color: Hsla| {
        (count > 0).then(|| {
            div()
                .flex_basis(px(0.))
                .flex_grow(count as f32)
                .min_w(px(3.))
                .rounded(px(2.))
                .bg(color)
                .h_full()
        })
    };
    div()
        .flex_none()
        .flex()
        .gap(px(2.))
        .h(px(6.))
        .when(wide, |bar| bar.w_full().mb(px(7.)))
        .when(!wide, |bar| bar.w(px(84.)))
        .children(part(progress.done, t.good))
        .children(part(progress.canceled, t.faint))
        .children(part(progress.open, t.line2))
        .when(progress.total() == 0, |bar| {
            bar.child(div().flex_1().rounded(px(2.)).bg(t.line2))
        })
}

fn stroke_arc(bounds: Bounds<Pixels>, sweep: f32, color: Hsla, window: &mut Window) {
    if sweep <= 0. {
        return;
    }
    let center = bounds.center();
    let mut path = PathBuilder::stroke(px(RING_WIDTH));
    for step in 0..=64 {
        let angle = TAU * sweep.min(1.) * step as f32 / 64.;
        let at = center
            + point(
                px(RING_RADIUS * angle.sin()),
                px(-RING_RADIUS * angle.cos()),
            );
        if step == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn ring_canvas(value: f32, color: Hsla, track: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            stroke_arc(bounds, 1., track, window);
            stroke_arc(bounds, value, color, window);
        },
    )
    .size(px(RING_SIZE))
}

fn span(delta: f32, from: f32, to: f32) -> f32 {
    ((delta - from) / (to - from)).clamp(0., 1.)
}

fn ease_out(delta: f32) -> f32 {
    1. - (1. - delta).powi(3)
}

impl Root {
    /// The ring beside an epic's ref. A local change sweeps it to the new value
    /// and the last child pops it; otherwise it rests.
    pub(crate) fn epic_ring(&self, row: &TaskRow, t: &Theme) -> Option<AnyElement> {
        let progress = row.epic.as_ref()?;
        let total = progress.total();
        let finished = total - progress.open;
        let resting = if total == 0 {
            0.
        } else {
            finished as f32 / total as f32
        };
        let label = |shown: usize, t: &Theme| {
            div()
                .font_family(MONO)
                .text_size(px(12.))
                .text_color(t.faint)
                .child(format!("{shown}/{total}"))
        };
        let (good, track) = (t.good, t.line2);
        let base = div().flex_none().flex().items_center().gap(px(6.));
        let Some((from, run)) = self.session.motion.ring(&row.id) else {
            return Some(
                base.child(ring_canvas(resting, good, track))
                    .child(label(finished, t))
                    .into_any_element(),
            );
        };
        let last = total > 0 && finished == total;
        let start = if total == 0 {
            0.
        } else {
            from as f32 / total as f32
        };
        let ring = div()
            .size(px(RING_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .with_animation(
                ("epic-ring", run),
                Animation::new(RING_DURATION),
                move |ring, delta| {
                    let value = start + (resting - start) * ease_out(span(delta, 0., 0.7));
                    let bump = if last {
                        (PI * span(delta, 0.6, 1.)).sin() * 0.16
                    } else {
                        0.
                    };
                    ring.size(px(RING_SIZE * (1. + bump)))
                        .child(ring_canvas(value, good, track))
                },
            );
        Some(
            base.child(ring)
                .child(label(finished, t))
                .into_any_element(),
        )
    }

    /// What is left of the list once the last open row goes.
    pub(crate) fn render_all_clear(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let logo = if t.dark {
            "logo-dark.svg"
        } else {
            "logo-light.svg"
        };
        let content = move |top: f32, opacity: f32| {
            div()
                .relative()
                .h(px(ALL_CLEAR_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .pt(px(30.))
                .top(px(top))
                .opacity(opacity)
                .text_size(px(12.))
                .text_color(t.faint)
                .child(gpui::img(logo).size(px(32.)))
                .child("All clear")
        };
        let Some(run) = self.session.motion.all_clear() else {
            return content(0., 0.8).into_any_element();
        };
        div()
            .child(content(0., 1.).with_animation(
                ("all-clear", run),
                Animation::new(ALL_CLEAR_DURATION),
                move |entry, delta| {
                    let shown = span(delta, 0.55, 0.9);
                    entry
                        .top(px(6. * (1. - ease_out(shown))))
                        .opacity(shown * 0.8)
                },
            ))
            .into_any_element()
    }

    pub(crate) fn toggle_mark(&mut self, _: &ToggleMark, _: &mut Window, cx: &mut Context<Self>) {
        self.session.toggle_selected_mark();
        self.arm_toast(cx);
        cx.notify();
    }

    fn extend_marks(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.session.selected.clone();
        let Some((current, next)) = current.zip(self.session.neighbour(delta)) else {
            return;
        };
        self.session.marks.insert(current);
        self.session.marks.insert(next.clone());
        self.select(next, window, cx);
        cx.notify();
    }

    pub(crate) fn extend_marks_down(
        &mut self,
        _: &ExtendMarksDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_marks(1, window, cx);
    }

    pub(crate) fn extend_marks_up(
        &mut self,
        _: &ExtendMarksUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extend_marks(-1, window, cx);
    }

    pub(crate) fn mark_visible(&mut self, _: &MarkVisible, _: &mut Window, cx: &mut Context<Self>) {
        self.session.mark_visible();
        self.arm_toast(cx);
        cx.notify();
    }

    pub(crate) fn clear_marks(&mut self, _: &ClearMarks, _: &mut Window, cx: &mut Context<Self>) {
        self.session.clear_marks();
        cx.notify();
    }

    /// What a row does with a click's modifiers. True means it was a mark.
    pub(crate) fn mark_click(
        &mut self,
        id: &TaskId,
        shift: bool,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if toggle {
            self.session.toggle_mark(id.clone());
        } else if shift {
            self.session.mark_range(id);
        } else {
            self.session.clear_marks();
            return false;
        }
        self.select(id.clone(), window, cx);
        cx.notify();
        true
    }

    pub(crate) fn new_epic(&mut self, _: &NewEpic, window: &mut Window, cx: &mut Context<Self>) {
        self.kept_draft = Draft {
            is_epic: true,
            ..std::mem::take(&mut self.kept_draft)
        };
        self.open_composer(window, cx);
    }

    pub(crate) fn new_recurring(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.kept_draft = Draft {
            schedule: "every friday at 09:00".to_string(),
            more: true,
            ..std::mem::take(&mut self.kept_draft)
        };
        self.open_composer(window, cx);
    }

    pub(crate) fn toggle_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closed = !self.session.snapshot.view.closed;
        self.switch_view(Command::SetClosed(closed), window, cx);
    }

    pub(crate) fn close_epic(&mut self, id: TaskId, cx: &mut Context<Self>) {
        self.session.set_status(&id, TaskStatus::Done);
        self.after_status_motion(cx);
    }

    pub(crate) fn reopen_epic(&mut self, id: TaskId, cx: &mut Context<Self>) {
        self.session.set_status(&id, TaskStatus::Todo);
        self.after_status_motion(cx);
    }

    pub(crate) fn jump_to_epic(
        &mut self,
        _: &JumpToEpic,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent = self
            .session
            .detail
            .as_ref()
            .and_then(|detail| detail.epic.clone());
        match parent {
            Some(epic) => self.follow(epic.id, window, cx),
            None => {
                self.session.show_toast("Not in an epic".to_string(), None);
                self.arm_toast(cx);
                cx.notify();
            }
        }
    }

    /// Children in the order `epicKids` walks them.
    fn visible_children(&self) -> Vec<Child> {
        let Some(detail) = &self.session.detail else {
            return Vec::new();
        };
        let finished = self.session.show_finished;
        [
            ChildGroup::Progress,
            ChildGroup::Ready,
            ChildGroup::Waiting,
            ChildGroup::Finished,
        ]
        .into_iter()
        .filter(|group| *group != ChildGroup::Finished || finished)
        .flat_map(|group| {
            detail
                .children
                .iter()
                .filter(move |child| child.group == group)
        })
        .cloned()
        .collect()
    }

    /// `c` steps into an epic's children, so `j`, `k` and task keys reach them.
    pub(crate) fn focus_child(&mut self, _: &FocusChild, _: &mut Window, cx: &mut Context<Self>) {
        let first = self
            .visible_children()
            .first()
            .map(|child| child.link.id.clone());
        self.session.child = match (&self.session.child, first) {
            (Some(_), _) => None,
            (None, Some(first)) => Some(first),
            (None, None) => {
                self.session
                    .show_toast("This epic has no tasks yet".to_string(), None);
                None
            }
        };
        self.arm_toast(cx);
        cx.notify();
    }

    /// Moves between an epic's children while one has focus.
    pub(crate) fn step_child(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(current) = self.session.child.clone() else {
            return false;
        };
        let children = self.visible_children();
        let at = children.iter().position(|child| child.link.id == current);
        let next = at.map_or(0, |at| {
            (at as isize + delta).clamp(0, children.len() as isize - 1) as usize
        });
        self.session.child = children.get(next).map(|child| child.link.id.clone());
        cx.notify();
        true
    }

    /// The epic's own cells in the Epics list.
    pub(crate) fn epic_cells(
        &self,
        row: &TaskRow,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let Some(progress) = &row.epic else {
            return Vec::new();
        };
        let closed = row.status.is_terminal();
        let closable = !closed && progress.open == 0 && progress.total() > 0;
        let reopenable = closed && progress.open > 0;
        let id = row.id.clone();
        let mut cells = Vec::new();
        if let Some(current) = &progress.current {
            cells.push(
                div()
                    .min_w_0()
                    .max_w(px(190.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(current.clone())
                    .into_any_element(),
            );
        }
        if closable || reopenable {
            let (name, element) = if closable {
                ("Close epic", "epic-close")
            } else {
                ("Reopen", "epic-reopen")
            };
            cells.push(
                btn(
                    gpui::ElementId::Name(format!("{element}-{}", row.id).into()),
                    name,
                    t,
                )
                .flex_none()
                .h(px(22.))
                .px(px(7.))
                .text_size(px(12.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if closable {
                        this.close_epic(id.clone(), cx);
                    } else {
                        this.reopen_epic(id.clone(), cx);
                    }
                }))
                .child(name)
                .into_any_element(),
            );
        }
        cells.push(epic_bar(progress, false, t).into_any_element());
        cells.push(
            div()
                .flex_none()
                .w(px(132.))
                .text_right()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(t.faint)
                .child(progress.counts())
                .into_any_element(),
        );
        cells
    }

    /// Enter on a series opens its current task.
    pub(crate) fn open_series_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current =
            (self.session.series_detail.as_ref()).and_then(|series| series.current.clone());
        if let Some(current) = current {
            self.session.goto(current.id);
            self.load_selection(window, cx);
            cx.notify();
        }
    }

    /// The series of the selected occurrence, in the Recurring list.
    pub(crate) fn open_series(
        &mut self,
        id: RecurrenceSeriesId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_task(window, cx);
        self.session.write(Command::OpenSeries(id));
        cx.notify();
    }

    /// `t c` puts the targets in an epic of their project, as `pickEpic` does.
    pub(crate) fn add_to_epic(
        &mut self,
        _: &AddToEpic,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.session.bulk_targets();
        let targets: Vec<TaskRow> = match &marked {
            Some(ids) => ids
                .iter()
                .filter_map(|id| self.session.snapshot.row(id).cloned())
                .collect(),
            None => self.session.target_row().cloned().into_iter().collect(),
        };
        let Some(first) = targets.first() else {
            return;
        };
        let project = first.project.clone();
        let same = targets
            .iter()
            .all(|row| row.project == project && !row.is_epic);
        let epics = self.session.catalog.iter().filter(|task| task.is_epic);
        let entries = epics
            .map(|epic| {
                let (id, marked) = (epic.id.clone(), marked.is_some());
                let own: Vec<TaskId> = targets.iter().map(|row| row.id.clone()).collect();
                let here = same && epic.project == project;
                Entry::new(epic.title.clone(), move |root, _, _| {
                    if !(marked && root.session.bulk(Bulk::Epic(id.clone()))) {
                        root.session
                            .write(Command::Bulk(own.clone(), Bulk::Epic(id.clone())));
                    }
                })
                .hint(if here {
                    epic.reference.clone()
                } else {
                    "Other project".to_string()
                })
                .off(!here)
            })
            .collect();
        let anchor = self.anchor("overflow");
        self.open_menu(
            entries,
            "Add to epic",
            anchor,
            MenuOptions::default(),
            window,
            cx,
        );
    }

    /// `Add existing`: open tasks of the epic's project that are not children.
    fn pick_child(&mut self, epic: TaskRow, window: &mut Window, cx: &mut Context<Self>) {
        let kids: Vec<TaskId> = self
            .session
            .detail
            .iter()
            .flat_map(|detail| detail.children.iter().map(|child| child.link.id.clone()))
            .collect();
        let tasks = self.session.catalog.iter().filter(|task| {
            task.project == epic.project
                && task.id != epic.id
                && !task.is_epic
                && !task.status.is_terminal()
                && !kids.contains(&task.id)
        });
        let entries = tasks
            .map(|task| {
                let (parent, child) = (epic.id.clone(), task.id.clone());
                Entry::new(task.title.clone(), move |root, _, _| {
                    root.session
                        .write(Command::AddChild(parent.clone(), child.clone()));
                })
                .hint(task.reference.clone())
                .mark(Mark::Status(task.status))
            })
            .collect();
        let anchor = self.anchor("add-child");
        self.open_menu(
            entries,
            "Add child from this project",
            anchor,
            MenuOptions::default(),
            window,
            cx,
        );
    }

    fn child_row(
        &self,
        epic: &TaskId,
        child: &Child,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        let focused = self.session.child.as_ref() == Some(&child.link.id);
        let (target, remove) = (child.link.id.clone(), child.link.id.clone());
        let epic = epic.clone();
        let raised = t.raised;
        div()
            .id(gpui::ElementId::Name(
                format!("kid-{}", child.link.id).into(),
            ))
            .role(Role::Link)
            .aria_label(format!("{} {}", child.link.reference, child.link.title))
            .flex()
            .items_center()
            .gap(px(7.))
            .min_h(px(27.))
            .px(px(4.))
            .ml(px(-4.))
            .rounded(px(5.))
            .hover(move |style| style.bg(raised))
            .when(focused, |row| row.bg(t.raised))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.follow(target.clone(), window, cx);
            }))
            .child(status_glyph(child.link.status, t))
            .child(ref_label(
                &child.link.reference,
                &child.link.project,
                None,
                t,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.body)
                    .child(child.link.title.clone()),
            )
            .when(!child.note.is_empty(), |row| {
                row.child(
                    small(11.)
                        .flex_none()
                        .max_w(px(160.))
                        .truncate()
                        .text_color(t.warn)
                        .child(child.note.clone()),
                )
            })
            .when(!child.due.is_empty(), |row| {
                row.child(
                    small(12.)
                        .flex_none()
                        .text_color(t.faint)
                        .child(child.due.clone()),
                )
            })
            .child(
                bare_btn(
                    gpui::ElementId::Name(format!("kid-remove-{}", child.link.id).into()),
                    "Remove",
                    t,
                )
                .flex_none()
                .text_color(t.faint)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.session
                        .write(Command::RemoveChild(epic.clone(), remove.clone()));
                }))
                .child("Remove"),
            )
    }

    /// The Tasks section of an epic, grouped as `epicSection` groups them.
    pub(crate) fn render_epic_section(
        &self,
        row: &TaskRow,
        detail: &Detail,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Div {
        let progress = row.epic.clone().unwrap_or(EpicProgress {
            done: 0,
            canceled: 0,
            open: 0,
            current: None,
        });
        let blocked = detail
            .children
            .iter()
            .filter(|child| !child.note.is_empty() && child.group != ChildGroup::Finished)
            .count();
        let epic = row.clone();
        let mut section = div()
            .mt(px(20.))
            .child(
                sub_head("Tasks", t).mt(px(0.)).child(div().flex_1()).child(
                    bare_btn("add-child", "Add existing", t)
                        .relative()
                        .text_color(t.muted)
                        .child(anchor_probe("add-child", &self.anchors))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_child(epic.clone(), window, cx);
                        }))
                        .child("Add existing"),
                ),
            )
            .child(epic_bar(&progress, true, t))
            .child(
                hint(
                    if blocked > 0 {
                        format!("{} · {blocked} waiting", progress.counts())
                    } else {
                        progress.counts()
                    },
                    t,
                )
                .mt(px(0.)),
            );
        for (group, name) in [
            (ChildGroup::Progress, "In progress"),
            (ChildGroup::Ready, "Ready"),
            (ChildGroup::Waiting, "Waiting"),
        ] {
            let children: Vec<&Child> = detail
                .children
                .iter()
                .filter(|child| child.group == group)
                .collect();
            if children.is_empty() {
                continue;
            }
            section = section.child(
                small(11.)
                    .mt(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .child(format!("{name} {}", children.len())),
            );
            for child in children {
                section = section.child(self.child_row(&row.id, child, cx, t));
            }
        }
        let finished: Vec<&Child> = detail
            .children
            .iter()
            .filter(|child| child.group == ChildGroup::Finished)
            .collect();
        if !finished.is_empty() {
            let open = self.session.show_finished;
            let name = format!(
                "{} Finished {}",
                if open { "▾" } else { "▸" },
                finished.len()
            );
            section = section.child(
                bare_btn("toggle-finished", "Finished", t)
                    .mt(px(10.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.session.show_finished = !this.session.show_finished;
                        cx.notify();
                    }))
                    .child(name),
            );
            if open {
                for child in finished {
                    section = section.child(self.child_row(&row.id, child, cx, t));
                }
            }
        }
        section.child(div().mt(px(8.)).child(self.kid_input.clone()))
    }

    pub(crate) fn on_kid_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Submit => self.submit_child(cx),
            EditorEvent::Cancel => window.focus(&self.focus, cx),
            _ => {}
        }
    }

    fn submit_child(&mut self, cx: &mut Context<Self>) {
        let title = self.kid_input.read(cx).text().trim().to_string();
        let epic = self
            .session
            .target_row()
            .filter(|row| row.is_epic)
            .map(|row| row.id.clone());
        if let Some(epic) = epic.filter(|_| !title.is_empty()) {
            self.session.write(Command::CreateChild(epic, title));
            self.kid_input
                .update(cx, |input, cx| input.set_text("", cx));
        }
    }

    /// The panel that stands in for one task while several are marked.
    pub(crate) fn render_bulk(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let rows: Vec<TaskRow> = self
            .session
            .bulk_targets()
            .unwrap_or_default()
            .iter()
            .filter_map(|id| self.session.snapshot.row(id).cloned())
            .collect();
        let count = if rows.len() == 1 {
            "1 task".to_string()
        } else {
            format!("{} tasks", rows.len())
        };
        let project = |row: &TaskRow| {
            let projects = &self.session.snapshot.projects;
            let found = projects
                .iter()
                .find(|project| project.prefix == row.project);
            found.map_or_else(|| row.project.clone(), |project| project.name.clone())
        };
        let labels = |row: &TaskRow| {
            if row.labels.is_empty() {
                "None".to_string()
            } else {
                row.labels.join(", ")
            }
        };
        let due = |row: &TaskRow| {
            if row.due.is_empty() {
                "None".to_string()
            } else {
                row.due.clone()
            }
        };
        let action = |id: &'static str, label: &'static str, t: &Theme| {
            line_btn(id, label, t)
                .relative()
                .child(anchor_probe(id, &self.anchors))
        };
        measured(div(), "detail")
            .flex_none()
            .w(px(width))
            .h_full()
            .ml(px(-1.))
            .flex()
            .flex_col()
            .bg(t.page)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(px(1.))
                    .bg(t.line),
            )
            .child(
                div()
                    .flex_none()
                    .min_h(px(40.))
                    .pl(px(18.))
                    .pr(px(10.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{count} marked")),
                    )
                    .child(div().flex_1())
                    .child(
                        btn("clear-marks", "Clear", &t)
                            .px(px(8.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.clear_marks(&ClearMarks, window, cx);
                            }))
                            .child("Clear"),
                    ),
            )
            .child(
                div()
                    .id("bulk-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pt(px(12.))
                    .px(px(18.))
                    .pb(px(40.))
                    .child(fact(
                        "Status",
                        mixed(&rows, |row| status_name(row.status).to_string()),
                        &t,
                    ))
                    .child(fact(
                        "Priority",
                        mixed(&rows, |row| priority_name(row.priority).to_string()),
                        &t,
                    ))
                    .child(fact("Project", mixed(&rows, project), &t))
                    .child(fact("Due", mixed(&rows, due), &t))
                    .child(fact("Labels", mixed(&rows, labels), &t))
                    .child(sub_head(format!("Apply to {count}"), &t))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(5.))
                            .mt(px(6.))
                            .child(action("status", "Status", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.change_status(&ChangeStatus, window, cx);
                                },
                            )))
                            .child(action("priority", "Priority", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.change_priority(&ChangePriority, window, cx);
                                },
                            )))
                            .child(
                                line_btn("bulk-schedule", "Schedule", &t).on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.edit_availability(&EditAvailability, window, cx);
                                    },
                                )),
                            )
                            .child(action("labels", "Labels", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.edit_labels(&EditLabels, window, cx);
                                },
                            )))
                            .child(action("project", "Project", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.change_project(&ChangeProject, window, cx);
                                },
                            )))
                            .child(action("overflow", "Epic", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.add_to_epic(&AddToEpic, window, cx);
                                },
                            )))
                            .child(line_btn("bulk-done", "Mark done", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.mark_done(&MarkDone, window, cx);
                                },
                            )))
                            .child(line_btn("bulk-cancel", "Cancel", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.cancel_task(&CancelTask, window, cx);
                                },
                            )))
                            .child(line_btn("bulk-delete", "Delete", &t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.delete_task(&DeleteTask, window, cx);
                                },
                            ))),
                    )
                    .child(sub_head("Tasks", &t))
                    .children(rows.iter().map(|row| {
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .min_h(px(24.))
                            .text_color(t.body)
                            .child(div().min_w_0().truncate().child(row.title.clone()))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(t.faint)
                                    .child(row.reference.clone()),
                            )
                    })),
            )
    }

    pub(crate) fn set_lifecycle(
        &mut self,
        lifecycle: Lifecycle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_view(Command::ShowRecurring(lifecycle), window, cx);
        let name = match lifecycle {
            Lifecycle::ActiveOrPaused => "active and paused",
            Lifecycle::Active => "active",
            Lifecycle::Paused => "paused",
            Lifecycle::Stopped => "stopped",
            Lifecycle::All => "all",
        };
        self.session.show_toast(format!("Recurring: {name}"), None);
        self.arm_toast(cx);
    }

    pub(crate) fn cycle_lifecycle(
        &mut self,
        _: &CycleLifecycle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = &self.session.snapshot.view;
        let at = LIFECYCLES
            .iter()
            .position(|(entry, _)| *entry == view.lifecycle);
        let next = at.map_or(0, |at| (at + 1) % LIFECYCLES.len());
        self.set_lifecycle(LIFECYCLES[next].0, window, cx);
    }

    pub(crate) fn render_lifecycle(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let current = self.session.snapshot.view.lifecycle;
        div()
            .h(px(LIFECYCLE_HEIGHT))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .children(LIFECYCLES.iter().map(|(lifecycle, name)| {
                let lifecycle = *lifecycle;
                let on = lifecycle == current;
                bare_btn(("life", lifecycle as usize), *name, &t)
                    .px(px(9.))
                    .rounded(px(12.))
                    .text_size(px(12.))
                    .text_color(if on { t.ink } else { t.muted })
                    .when(on, |pill| pill.bg(t.tint))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.switch_view(Command::ShowRecurring(lifecycle), window, cx);
                    }))
                    .child(*name)
            }))
            .into_any_element()
    }

    fn series_cells(
        row: Div,
        reference: impl IntoElement,
        title: impl IntoElement,
        rule: impl IntoElement,
        next: impl IntoElement,
        state: impl IntoElement,
    ) -> Div {
        row.flex()
            .items_center()
            .gap(px(10.))
            .px(px(8.))
            .child(div().flex_none().w(px(96.)).child(reference))
            .child(div().flex_1().min_w_0().truncate().child(title))
            .child(div().flex_none().w(px(190.)).truncate().child(rule))
            .child(div().flex_none().w(px(84.)).child(next))
            .child(div().flex_none().w(px(60.)).child(state))
    }

    pub(crate) fn render_series_head(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let head = Self::series_cells(
            small(11.)
                .h(px(SERIES_HEAD_HEIGHT))
                .mx(px(8.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.faint),
            "Ref",
            "Title",
            "Schedule",
            "Next",
            "State",
        );
        div().child(head).into_any_element()
    }

    pub(crate) fn render_series_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(row): Option<&SeriesRow> = self.session.snapshot.series.get(index) else {
            return div().into_any_element();
        };
        let selected = self.session.series.as_ref() == Some(&row.id);
        let id = row.id.clone();
        let next = row.after.as_deref().map_or("-".to_string(), |date| {
            crate::adapter::due_text(date, chrono::Local::now().date_naive())
        });
        let panel = t.panel;
        let cells = Self::series_cells(
            div()
                .h(px(SERIES_ROW_HEIGHT))
                .mx(px(8.))
                .rounded(px(6.))
                .when(selected, |row| {
                    row.bg(t.tint).shadow(vec![t.inset_ring(t.tint_line)])
                }),
            ref_label(&row.reference, &row.project, None, &t),
            row.title.clone(),
            small(12.)
                .text_color(t.faint)
                .truncate()
                .child(row.rule.clone()),
            small(12.).text_color(t.faint).child(next),
            small(11.)
                .text_color(state_color(row.state, &t))
                .child(state_name(row.state)),
        );
        div()
            .id(("series", index))
            .role(Role::ListItem)
            .aria_label(row.title.clone())
            .aria_selected(selected)
            .child(cells)
            .hover(move |style| style.bg(panel))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.session.select_series(id.clone());
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Stops a series, asking first when it has a current task to settle.
    pub(crate) fn stop_series(
        &mut self,
        id: RecurrenceSeriesId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = self.session.snapshot.series.iter().find(|row| row.id == id);
        let current = row.and_then(|row| row.current.clone());
        let title = row.map_or_else(String::new, |row| row.title.clone());
        match current {
            Some(current) => {
                let dialog = Dialog::Stop {
                    id,
                    title,
                    current: current.reference,
                };
                self.open_dialog(dialog, Vec::new(), window, cx);
            }
            None => self.session.write(Command::StopSeries(id, false)),
        }
    }

    fn replace_series(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.session.series_detail.clone() else {
            return;
        };
        let row = self
            .session
            .snapshot
            .series
            .iter()
            .find(|row| row.id == detail.id);
        let project = row.map(|row| row.project.clone());
        self.kept_draft = Draft {
            title: detail.title,
            body: detail.description,
            project,
            schedule: detail.rule.to_lowercase(),
            replaces: Some(detail.id),
            more: true,
            ..Draft::default()
        };
        self.open_composer(window, cx);
    }

    fn edit_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.session.series_detail.clone() else {
            return;
        };
        let id = detail.id;
        self.open_prompt_in(
            Mode::Multi,
            "Edit template".to_string(),
            "Description for later tasks",
            "",
            &detail.description,
            std::rc::Rc::new(move |root, text, _, _| {
                root.session.write(Command::SetTemplate(id.clone(), text));
            }),
            window,
            cx,
        );
    }

    pub(crate) fn render_series(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let panel = measured(div(), "detail")
            .flex_none()
            .w(px(width))
            .h_full()
            .ml(px(-1.))
            .flex()
            .flex_col()
            .bg(t.page)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(px(1.))
                    .bg(t.line),
            );
        let Some(series) = self.session.series_detail.clone() else {
            return panel.child(
                div()
                    .m_auto()
                    .text_color(t.faint)
                    .child("Select a recurring task."),
            );
        };
        let id = series.id.clone();
        let today = chrono::Local::now().date_naive();
        let open = series.state != RecurrenceSeriesState::Stopped;
        let write = |id: &'static str,
                     label: &'static str,
                     command: Command,
                     t: &Theme,
                     cx: &mut Context<Self>| {
            line_btn(id, label, t).on_click(cx.listener(move |this, _, _, cx| {
                this.session.write(command.clone());
                cx.notify();
            }))
        };
        let stop_id = id.clone();
        let quick = div()
            .flex()
            .flex_wrap()
            .gap(px(5.))
            .mt(px(10.))
            .when(series.state == RecurrenceSeriesState::Active, |quick| {
                quick.child(write(
                    "rec-pause",
                    "Pause",
                    Command::PauseSeries(id.clone()),
                    &t,
                    cx,
                ))
            })
            .when(series.state == RecurrenceSeriesState::Paused, |quick| {
                quick.child(write(
                    "rec-resume",
                    "Resume",
                    Command::ResumeSeries(id.clone()),
                    &t,
                    cx,
                ))
            })
            .when(series.skippable, |quick| {
                quick.child(write(
                    "rec-skip",
                    "Skip current",
                    Command::SkipSeries(id.clone()),
                    &t,
                    cx,
                ))
            })
            .when(open, |quick| {
                quick
                    .child(line_btn("rec-stop", "Stop", &t).text_color(t.bad).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.stop_series(stop_id.clone(), window, cx);
                        }),
                    ))
                    .child(line_btn("rec-replace", "Stop and replace", &t).on_click(
                        cx.listener(|this, _, window, cx| this.replace_series(window, cx)),
                    ))
            });
        let current = series.current.clone().map(|current| {
            let target = current.id.clone();
            let slot = crate::adapter::due_text(&current.slot, today);
            div().child(sub_head("Current task", &t)).child(
                btn("rec-current", current.reference.clone(), &t)
                    .w_full()
                    .gap(px(7.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.session.goto(target.clone());
                        this.load_selection(window, cx);
                        cx.notify();
                    }))
                    .child(div().text_color(t.faint).child(current.reference))
                    .child(div().flex_1().truncate().child(slot)),
            )
        });
        let body = if series.description.is_empty() {
            hint("No template text", &t).mt(px(0.)).into_any_element()
        } else {
            div()
                .text_color(t.body)
                .child(markdown::render(&series.description).text)
                .into_any_element()
        };
        panel
            .child(
                div()
                    .flex_none()
                    .min_h(px(40.))
                    .pl(px(18.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(series.reference.clone()),
                    ),
            )
            .child(
                div()
                    .id("series-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pt(px(12.))
                    .px(px(18.))
                    .pb(px(40.))
                    .child(
                        div()
                            .mb(px(10.))
                            .text_size(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(series.title.clone()),
                    )
                    .child(
                        fact("State", state_name(series.state), &t)
                            .text_color(state_color(series.state, &t)),
                    )
                    .child(fact("Schedule", series.rule.clone(), &t))
                    .child(fact("Due", series.due.clone(), &t))
                    .child(fact("Time zone", series.timezone.clone(), &t))
                    .child(fact(
                        "After current",
                        series
                            .after
                            .as_deref()
                            .map_or("-".to_string(), |date| crate::adapter::due_text(date, today)),
                        &t,
                    ))
                    .child(hint("Pattern, start date and time zone can't change after creation.", &t))
                    .child(quick)
                    .when(series.state == RecurrenceSeriesState::Active, |body| {
                        body.child(hint("Pause hides the current task until you resume.", &t))
                    })
                    .children(current)
                    .child(
                        sub_head("Template", &t).child(div().flex_1()).child(
                            bare_btn("rec-template", "Edit", &t)
                                .text_color(t.muted)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.edit_template(window, cx);
                                }))
                                .child("Edit"),
                        ),
                    )
                    .child(body)
                    .child(hint("Edits apply to later tasks. The current task and history stay as they are.", &t))
                    .child(sub_head("History", &t))
                    .when(series.history.is_empty(), |body| {
                        body.child(hint("No dates yet", &t).mt(px(0.)))
                    })
                    .children(series.history.iter().map(|(date, outcome)| {
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .min_h(px(24.))
                            .text_color(t.body)
                            .child(if date.is_empty() {
                                "-".to_string()
                            } else {
                                crate::adapter::due_text(date, today)
                            })
                            .child(div().text_color(t.faint).child(outcome.clone()))
                    })),
            )
    }

    /// A task's conflicts over the details, captured so each field stays the
    /// one the user is looking at.
    pub(crate) fn open_resolve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(targets) = self
            .session
            .detail
            .as_ref()
            .map(|detail| detail.conflicts.clone())
            .filter(|targets| !targets.is_empty())
        else {
            self.session.show_toast(
                "selected task has no unresolved conflicts".to_string(),
                None,
            );
            self.arm_toast(cx);
            cx.notify();
            return;
        };
        self.show_resolve(targets, 0, window, cx);
    }

    fn show_resolve(
        &mut self,
        targets: Vec<ConflictTarget>,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| Editor::new(Mode::Multi, "", window, cx).enter_breaks());
        let dialog = Dialog::Resolve {
            targets,
            at,
            input: input.clone(),
            chosen: None,
        };
        self.open_dialog(dialog, vec![input], window, cx);
    }

    fn pick_side(&mut self, side: usize, cx: &mut Context<Self>) {
        if let Some(layer) = self.dialog.as_mut()
            && let Dialog::Resolve {
                targets,
                at,
                input,
                chosen,
            } = &mut layer.dialog
        {
            let target = &targets[*at];
            let value = if side == 0 {
                target.local_value.clone()
            } else {
                target.remote_value.clone()
            };
            *chosen = Some(side);
            input.update(cx, |input, cx| input.set_text(&value, cx));
            cx.notify();
        }
    }

    /// The field's one value, then the next field or the end. Resolving a field
    /// never touches another, and never reads marks.
    fn finish_resolve(&mut self, more: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Resolve {
            targets,
            at,
            input,
            chosen,
        }) = self.dialog.as_ref().map(|layer| &layer.dialog)
        else {
            return;
        };
        let value = input.read(cx).text().to_string();
        if chosen.is_none() && value.trim().is_empty() {
            return;
        }
        let (targets, at) = (targets.clone(), *at);
        let command = Command::ResolveConflict(Box::new(targets[at].clone()), value);
        self.session.write(command);
        self.close_dialog(window, cx);
        if more && at + 1 < targets.len() {
            self.show_resolve(targets, at + 1, window, cx);
        }
    }

    pub(crate) fn render_resolve(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Dialog::Resolve {
            targets,
            at,
            input,
            chosen,
        }) = self.dialog.as_ref().map(|layer| &layer.dialog)
        else {
            return Vec::new();
        };
        let target = &targets[*at];
        let reference = target.display_ref.clone();
        let label = field_label(&target.field);
        let typed = chosen.is_some() || !input.read(cx).text().trim().is_empty();
        let more = *at + 1 < targets.len();
        let written = matches!(target.field.as_str(), "title" | "description");
        let side = |index: usize, device: &str, value: &str, cx: &mut Context<Self>| {
            let on = *chosen == Some(index);
            div()
                .id(("pair", index))
                .role(Role::Button)
                .aria_label(format!("Keep {value}"))
                .flex_1()
                .min_w_0()
                .p(px(8.))
                .rounded(px(7.))
                .shadow(vec![t.inset_ring(if on { t.accent } else { t.line })])
                .on_click(cx.listener(move |this, _, _, cx| this.pick_side(index, cx)))
                .child(small(11.).text_color(t.faint).child(device.to_string()))
                .child(div().mt(px(4.)).text_color(t.body).child(value.to_string()))
        };
        vec![
            div()
                .px(px(16.))
                .pt(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("{label} on {reference}"))
                .into_any_element(),
            div()
                .px(px(16.))
                .py(px(10.))
                .child(hint("Both devices changed this field before syncing. Both values are kept until you choose.", t).mt(px(0.)))
                .child(
                    div()
                        .mt(px(8.))
                        .flex()
                        .gap(px(10.))
                        .child(side(0, &target.variant_a, &target.local_value, cx))
                        .child(side(1, &target.variant_b, &target.remote_value, cx)),
                )
                .when(written, |body| {
                    body.child(
                        small(12.)
                            .mt(px(10.))
                            .mb(px(4.))
                            .text_color(t.muted)
                            .child("Or write a final value"),
                    )
                    .child(
                        div()
                            .id("r-v")
                            .min_h(px(90.))
                            .p(px(8.))
                            .rounded(px(7.))
                            .shadow(vec![t.inset_ring(t.line)])
                            .child(input.clone()),
                    )
                })
                .into_any_element(),
            div()
                .px(px(16.))
                .pb(px(14.))
                .flex()
                .items_center()
                .gap(px(6.))
                .when(targets.len() > 1, |foot| {
                    foot.child(small(12.).text_color(t.faint).child(format!(
                        "Field {} of {}",
                        at + 1,
                        targets.len()
                    )))
                })
                .child(div().flex_1())
                .child(
                    btn("resolve-later", "Decide later", t)
                        .px(px(8.))
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
                        .child("Decide later"),
                )
                .when(more, |foot| {
                    foot.child(
                        line_btn("resolve-next", "Keep and next", t)
                            .when(!typed, |button| button.opacity(0.5))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.finish_resolve(true, window, cx);
                            })),
                    )
                })
                .child(
                    fill_btn("resolve-ok", "Keep this value", t)
                        .when(!typed, |button| button.opacity(0.5))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.finish_resolve(false, window, cx);
                        })),
                )
                .into_any_element(),
        ]
    }

    pub(crate) fn render_stop(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Dialog::Stop { id, title, current }) =
            self.dialog.as_ref().map(|layer| &layer.dialog)
        else {
            return Vec::new();
        };
        let (keep, skip) = (id.clone(), id.clone());
        vec![
            div()
                .px(px(16.))
                .pt(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("Stop {title}?"))
                .into_any_element(),
            div()
                .px(px(16.))
                .py(px(10.))
                .text_color(t.body)
                .child(format!(
                    "No more tasks will be created. What about the current task, {current}?"
                ))
                .into_any_element(),
            div()
                .px(px(16.))
                .pb(px(14.))
                .flex()
                .justify_end()
                .gap(px(6.))
                .child(
                    btn("stop-cancel", "Cancel", t)
                        .px(px(8.))
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
                        .child("Cancel"),
                )
                .child(line_btn("stop-keep", "Keep it", t).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.session.write(Command::StopSeries(keep.clone(), false));
                        this.close_dialog(window, cx);
                    },
                )))
                .child(line_btn("stop-skip", "Skip it", t).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.session.write(Command::StopSeries(skip.clone(), true));
                        this.close_dialog(window, cx);
                    },
                )))
                .into_any_element(),
        ]
    }

    pub(crate) fn publish_gist(
        &mut self,
        _: &PublishGist,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.session.target().cloned() {
            self.open_dialog(Dialog::Gist { id }, Vec::new(), window, cx);
        }
    }

    pub(crate) fn render_gist(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Dialog::Gist { id }) = self.dialog.as_ref().map(|layer| &layer.dialog) else {
            return Vec::new();
        };
        let id = id.clone();
        vec![
            div()
                .px(px(16.))
                .pt(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Create GitHub gist")
                .into_any_element(),
            div()
                .px(px(16.))
                .py(px(10.))
                .text_color(t.body)
                .child(
                    "Publish this task as a secret GitHub gist? Anyone with the URL can view it.",
                )
                .into_any_element(),
            div()
                .px(px(16.))
                .pb(px(14.))
                .flex()
                .justify_end()
                .gap(px(6.))
                .child(
                    btn("gist-cancel", "Cancel", t)
                        .px(px(8.))
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
                        .child("Cancel"),
                )
                .child(fill_btn("gist-ok", "Publish", t).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.close_dialog(window, cx);
                        this.session.publish_gist(id.clone());
                        this.arm_toast(cx);
                    },
                )))
                .into_any_element(),
        ]
    }

    pub(crate) fn review_tasks(
        &mut self,
        _: &ReviewTasks,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapse(window, cx);
        self.session.start_review();
        cx.notify();
    }

    pub(crate) fn review_decide(
        &mut self,
        decide: &ReviewDecide,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let status = match decide.0 {
            't' => TaskStatus::Todo,
            'b' => TaskStatus::Backlog,
            'a' => TaskStatus::Active,
            'd' => TaskStatus::Done,
            _ => TaskStatus::Canceled,
        };
        self.session.review_status(decide.0, status);
        self.arm_toast(cx);
        self.after_status_motion(cx);
    }

    pub(crate) fn review_keep(&mut self, _: &ReviewKeep, _: &mut Window, cx: &mut Context<Self>) {
        self.session.review_keep();
        self.arm_toast(cx);
        cx.notify();
    }

    pub(crate) fn review_skip(&mut self, _: &ReviewSkip, _: &mut Window, cx: &mut Context<Self>) {
        self.session.review_skip();
        cx.notify();
    }

    pub(crate) fn review_back(&mut self, _: &ReviewBack, _: &mut Window, cx: &mut Context<Self>) {
        self.session.review_back();
        cx.notify();
    }

    pub(crate) fn review_undo(&mut self, _: &ReviewUndo, _: &mut Window, cx: &mut Context<Self>) {
        self.session.review_undo();
        cx.notify();
    }

    pub(crate) fn review_exit(
        &mut self,
        _: &ReviewExit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session.exit_review();
        self.load_selection(window, cx);
        self.arm_toast(cx);
        cx.notify();
    }

    pub(crate) fn review_next(&mut self, _: &ReviewNext, _: &mut Window, cx: &mut Context<Self>) {
        let summary = self
            .session
            .review
            .as_ref()
            .is_some_and(|review| review.card().is_none());
        if summary {
            self.session.next_review_page();
            cx.notify();
        }
    }

    fn review_summary(&self, review: &Review, cx: &mut Context<Self>, t: &Theme) -> Div {
        let decided = review.decided.len();
        let skipped = review.cards.len() - decided;
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .py(px(40.))
            .text_color(t.muted)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.ink)
                    .child(format!("{decided} decided, {skipped} skipped")),
            )
            .child(if review.more > 0 {
                format!("{} more waiting.", review.more)
            } else {
                "Nothing else needs a decision.".to_string()
            })
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .when(review.more > 0, |row| {
                        row.child(
                            fill_btn("review-next", "Next batch ↵", t).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.review_next(&ReviewNext, window, cx);
                                },
                            )),
                        )
                    })
                    .child(line_btn("review-done", "Done", t).on_click(cx.listener(
                        |this, _, window, cx| this.review_exit(&ReviewExit, window, cx),
                    ))),
            )
    }

    fn review_key(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        key: &'static str,
        t: &Theme,
    ) -> Stateful<Div> {
        let label = label.into();
        btn(id, label.clone(), t)
            .gap(px(6.))
            .px(px(8.))
            .shadow(vec![t.inset_ring(t.line)])
            .child(label)
            .child(kbd(key, t.faint))
    }

    pub(crate) fn render_review(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let page = div()
            .id("review")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .pt(px(30.))
            .px(px(24.))
            .pb(px(50.))
            .flex()
            .flex_col()
            .items_center();
        let column = div().w_full().max_w(px(640.));
        let Some(review) = &self.session.review else {
            return page;
        };
        let Some(card) = review.card().cloned() else {
            return page.child(column.child(self.review_summary(review, cx, &t)));
        };
        let total = review.cards.len();
        let at = review.at;
        let row = self
            .session
            .target_row()
            .filter(|row| row.id == card.id)
            .cloned();
        let description = self
            .session
            .detail
            .as_ref()
            .filter(|detail| detail.task_id == card.id)
            .map(|detail| detail.description.clone())
            .unwrap_or_default();
        let eyebrow =
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .text_size(px(12.))
                .text_color(t.faint)
                .child(format!("Card {} of {total}", at + 1))
                .child(
                    div().flex_1().h(px(3.)).rounded(px(3.)).bg(t.line).child(
                        div()
                            .h_full()
                            .rounded(px(3.))
                            .bg(t.accent)
                            .w(gpui::relative(at as f32 / total as f32)),
                    ),
                )
                .when(at > 0, |bar| {
                    bar.child(
                        self.review_key("review-back", "Back", "p", &t)
                            .shadow(Vec::new())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.review_back(&ReviewBack, window, cx)
                            })),
                    )
                })
                .child(
                    self.review_key("review-exit", "Exit", "esc", &t)
                        .shadow(Vec::new())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.review_exit(&ReviewExit, window, cx)
                        })),
                );
        let Some(row) = row else {
            return page.child(column.child(eyebrow));
        };
        let status_key = |id,
                          status: TaskStatus,
                          name: &'static str,
                          key: char,
                          label,
                          cx: &mut Context<Self>| {
            (row.status != status).then(|| {
                self.review_key(id, name, label, &t).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.review_decide(&ReviewDecide(key), window, cx);
                    },
                ))
            })
        };
        let decide = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .mt(px(18.))
            .pt(px(14.))
            .border_t_1()
            .border_color(t.line)
            .when(card.stale, |decide| {
                decide.child(
                    self.review_key(
                        "review-keep",
                        format!("Keep {}", status_name(row.status)),
                        "k",
                        &t,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.review_keep(&ReviewKeep, window, cx);
                    })),
                )
            })
            .children(status_key(
                "review-todo",
                TaskStatus::Todo,
                "Todo",
                't',
                "t",
                cx,
            ))
            .children(status_key(
                "review-backlog",
                TaskStatus::Backlog,
                "Backlog",
                'b',
                "b",
                cx,
            ))
            .children(status_key(
                "review-active",
                TaskStatus::Active,
                "Active",
                'a',
                "a",
                cx,
            ))
            .child(
                self.review_key("priority", "Priority", "!", &t)
                    .relative()
                    .child(anchor_probe("priority", &self.anchors))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.change_priority(&ChangePriority, window, cx);
                    })),
            )
            .child(
                self.review_key("review-defer", "Defer", "e", &t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.edit_availability(&EditAvailability, window, cx)
                    })),
            )
            .children(status_key(
                "review-done",
                TaskStatus::Done,
                "Done",
                'd',
                "d",
                cx,
            ))
            .children(status_key(
                "review-cancel",
                TaskStatus::Canceled,
                "Cancel",
                'x',
                "x",
                cx,
            ))
            .child(
                self.review_key(
                    "review-skip",
                    if row.recurring {
                        "Skip this review"
                    } else {
                        "Skip"
                    },
                    "space",
                    &t,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.review_skip(&ReviewSkip, window, cx);
                })),
            );
        let body = if description.is_empty() {
            hint("No description", &t).into_any_element()
        } else {
            self.description.clone().into_any_element()
        };
        page.child(
            column
                .child(eyebrow)
                .child(
                    div()
                        .mt(px(16.))
                        .mb(px(8.))
                        .text_size(px(24.))
                        .line_height(px(29.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.ink)
                        .child(row.title.clone()),
                )
                .child(
                    small(12.)
                        .mb(px(12.))
                        .text_color(t.warn)
                        .child(card.why.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(14.))
                        .items_center()
                        .mb(px(12.))
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(status_glyph(row.status, &t))
                                .child(status_name(row.status)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(priority_icon(row.priority, &t))
                                .child(priority_name(row.priority)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(project_dot(&row.project, &t))
                                .child(row.project.clone()),
                        )
                        .child(ref_label(&row.reference, &row.project, None, &t))
                        .when(!row.due.is_empty(), |facts| {
                            facts.child(format!("Due {}", row.due))
                        }),
                )
                .child(body)
                .child(decide),
        )
    }

    /// The banner over a task that differs between devices.
    pub(crate) fn conflict_banner(
        &self,
        detail: &Detail,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Option<impl IntoElement + use<>> {
        let text = match detail.conflicts.as_slice() {
            [] => return None,
            [one] => format!("{} differs between devices", field_label(&one.field)),
            many => format!("{} fields differ", many.len()),
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .mb(px(10.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(7.))
                .bg(t.warn.opacity(0.12))
                .text_color(t.warn)
                .child(text)
                .child(div().flex_1())
                .child(
                    bare_btn("resolve-review", "Review", t)
                        .h(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.warn)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_resolve(window, cx);
                        }))
                        .child("Review"),
                ),
        )
    }
}

fn field_label(field: &str) -> String {
    let mut letters = field.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

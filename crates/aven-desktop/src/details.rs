//! The details panel for the selected task.
use accesskit::Role;
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, FontWeight, Stateful, Window, div,
    prelude::*, px,
};

use crate::actions::*;
use crate::adapter::TaskRow;
use crate::kit::*;
use crate::measure::measured;
use crate::session::COPIED_DURATION;
use crate::shell::Root;
use crate::theme::{Theme, theme};

const SPLIT_OVERLAP: f32 = 1.;

impl Root {
    /// The ref in the header, confirming a copy before it returns.
    fn detail_ref(&self, row: &TaskRow, cx: &Context<Self>, t: &Theme) -> AnyElement {
        let label = ref_label(&row.reference, &row.project, None, t).into_any_element();
        let Some(run) = self.session.motion.copy(&row.id) else {
            return label;
        };
        let copied = || {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(t.good)
                .child(status_glyph(aven_core::choices::TaskStatus::Done, t))
                .child("Copied")
        };
        // Without motion the confirmation holds still until it expires.
        if cx.reduce_motion() {
            return copied().into_any_element();
        }
        let shown =
            |delta: f32| (delta / 0.08).min(1.) * (1. - ((delta - 0.86) / 0.14).clamp(0., 1.));
        let animation = Animation::new(COPIED_DURATION);
        div()
            .relative()
            .child(div().child(label).with_animation(
                ("ref-out", run),
                animation.clone(),
                move |label, delta| label.opacity(1. - shown(delta)),
            ))
            .child(copied().absolute().top_0().left_0().with_animation(
                ("ref-in", run),
                animation,
                move |copied, delta| copied.opacity(shown(delta)),
            ))
            .into_any_element()
    }

    fn prop(&self, id: &'static str, label: &'static str, t: &Theme) -> Stateful<Div> {
        let raised = t.raised;
        div()
            .id(id)
            .relative()
            .flex()
            .items_center()
            .w_full()
            .min_h(px(27.))
            .px(px(4.))
            .ml(px(-4.))
            .rounded(px(5.))
            .text_color(t.body)
            .hover(move |style| style.bg(raised))
            .child(
                div()
                    .flex_none()
                    .w(px(84.))
                    .text_color(t.faint)
                    .child(label),
            )
    }

    pub(crate) fn render_detail(
        &self,
        width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let panel = measured(div(), "detail")
            .flex_none()
            .w(px(width))
            .h_full()
            .ml(px(-SPLIT_OVERLAP))
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
        let selected = self.session.selected.as_ref();
        let Some(row) = selected
            .and_then(|id| self.session.snapshot.row(id))
            .cloned()
        else {
            return panel;
        };
        let title_focused = self.title.read(cx).focus.is_focused(window);
        let note_focused = self.note.read(cx).focus.is_focused(window);
        let done = row.status == aven_core::choices::TaskStatus::Done;
        let anchors = self.anchors.clone();
        let detail = self.session.detail.as_ref();

        let top = div()
            .flex_none()
            .min_h(px(40.))
            .pl(px(18.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(4.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .id("detail-ref")
                    .role(Role::Button)
                    .aria_label("Copy ref")
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.copy_ref(&CopyRef, window, cx)),
                    )
                    .child(self.detail_ref(&row, cx, &t)),
            )
            .child(div().flex_1())
            .child(
                btn("mark-done", "Mark done", &t)
                    .px(px(8.))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.mark_done(&MarkDone, window, cx)),
                    )
                    .child(if done { "Reopen" } else { "Mark done" }),
            );

        let value = || div().flex().items_center().gap(px(7.)).min_w_0();
        let plain = |text: String| value().child(div().truncate().child(text));
        let available = detail.map_or(String::new(), |detail| detail.available.clone());
        let project = detail
            .and_then(|detail| {
                let projects = &self.session.snapshot.projects;
                projects
                    .iter()
                    .find(|project| project.key == detail.project)
            })
            .map_or(String::new(), |project| project.name.clone());
        let props = div()
            .child(
                self.prop("status", "Status", &t)
                    .role(Role::Button)
                    .aria_label("Status")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.change_status(&ChangeStatus, window, cx)
                    }))
                    .child(anchor_probe("status", &anchors))
                    .child(
                        value()
                            .child(self.task_glyph(&row.id, row.status, &t))
                            .child(status_name(row.status)),
                    ),
            )
            .child(
                self.prop("priority", "Priority", &t)
                    .role(Role::Button)
                    .aria_label("Priority")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.change_priority(&ChangePriority, window, cx)
                    }))
                    .child(anchor_probe("priority", &anchors))
                    .child(
                        value()
                            .child(self.task_priority(
                                &row.id,
                                row.priority,
                                ("pri-detail-old", "pri-detail-new"),
                                &t,
                            ))
                            .child(priority_name(row.priority)),
                    ),
            )
            .child(
                self.prop("available", "Available", &t)
                    .child(plain(available)),
            )
            .child(
                self.prop("due", "Due", &t)
                    .child(plain(if row.due.is_empty() {
                        "None".to_string()
                    } else {
                        row.due.clone()
                    })),
            )
            .child(
                self.prop("project", "Project", &t).child(
                    value()
                        .child(
                            div()
                                .size(px(8.))
                                .rounded(px(3.))
                                .bg(t.project_dot(&row.project)),
                        )
                        .child(project),
                ),
            )
            .child(
                self.prop("labels", "Labels", &t)
                    .child(plain(if row.labels.is_empty() {
                        "None".to_string()
                    } else {
                        row.labels.join(", ")
                    })),
            );

        let sub = |heading: &'static str| {
            div().relative().mt(px(20.)).child(
                div()
                    .mb(px(6.))
                    .text_size(px(12.))
                    .line_height(px(16.8))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.muted)
                    .child(heading),
            )
        };

        let mut notes = sub("Notes");
        for (note, view) in &self.note_views {
            notes = notes.child(
                div()
                    .mb(px(10.))
                    .text_color(t.body)
                    .line_height(px(19.5))
                    .child(
                        small(11.)
                            .line_height(px(15.4))
                            .text_color(t.faint)
                            .child(note.at.clone()),
                    )
                    .child(view.clone()),
            );
        }
        notes = notes.child(
            div()
                .id("note-field")
                .w_full()
                .min_h(px(32.))
                .px(px(9.))
                .py(px(7.))
                .rounded(px(6.))
                .bg(t.page)
                .shadow(vec![t.inset_ring(if note_focused {
                    t.tint_line
                } else {
                    t.line
                })])
                .text_color(t.ink)
                .line_height(px(UI_LINE_HEIGHT))
                .child(self.note.clone()),
        );

        let body = div()
            .id("detail-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.detail_scroll)
            .pt(px(12.))
            .px(px(18.))
            .pb(px(40.))
            .child(
                div()
                    .id("title-field")
                    .mt(px(2.))
                    .mb(px(12.))
                    .ml(px(-4.))
                    .px(px(4.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .text_size(px(19.))
                    .line_height(px(24.7))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.ink)
                    .hover(|style| style.bg(t.raised))
                    .when(title_focused, |field| {
                        field.bg(t.raised).shadow(vec![t.ring(t.tint_line)])
                    })
                    .child(self.title.clone()),
            )
            .child(props)
            .child(
                sub("Description").child(
                    div()
                        .text_color(t.body)
                        .line_height(px(20.15))
                        .min_h(px(20.))
                        .child(self.description.clone()),
                ),
            )
            .child(notes);

        panel.child(top).child(body)
    }
}

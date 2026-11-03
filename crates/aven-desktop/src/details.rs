//! A task's content: the details panel beside the list, and the same parts laid
//! out as a document when the task is expanded.
use accesskit::Role;
use aven_core::choices::TaskStatus;
use aven_core::ids::TaskId;
use gpui::{
    AnyElement, Context, Div, Entity, ExternalPaths, FontWeight, ObjectFit, Stateful, Window, div,
    img, prelude::*,
};

use crate::actions::*;
use crate::adapter::{Command, Detail, Link, TaskRow};
use crate::commands::pasted_image;
use crate::input::{Editor, EditorEvent};
use crate::kit::*;
use crate::schedule::DateField;
use crate::shell::{BodyEdit, Root};
use crate::theme::{Theme, theme};

const SPLIT_OVERLAP: f32 = 1.;
const THUMB_WIDTH: f32 = 96.;
const THUMB_HEIGHT: f32 = 68.;
const DOC_WIDTH: f32 = 1040.;
const DOC_SIDE_WIDTH: f32 = 260.;
const META_KEY_WIDTH: f32 = 100.;
/// Narrower than this, the fields stack in one column.
const PROP_MIN_WIDTH: f32 = 220.;
const ACTIVITY_LINE_HEIGHT: f32 = 18.;
const ACTIVITY_DOT: f32 = 5.;
const ROW_GROUP: &str = "task-row";

fn section_head(heading: &'static str, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .mb(px(6.))
        .text_size(px(12.))
        .line_height(px(16.8))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.muted)
        .child(heading)
}

/// A row's action, shown while the pointer is on the row.
fn row_action(id: impl Into<gpui::ElementId>, label: &'static str, t: &Theme) -> Stateful<Div> {
    side_action(id, label, t)
        .invisible()
        .group_hover(ROW_GROUP, |style| style.visible())
}

fn side_action(id: impl Into<gpui::ElementId>, label: &'static str, t: &Theme) -> Stateful<Div> {
    bare_btn(id, label, t)
        .h(px(20.))
        .px(px(4.))
        .text_size(px(12.))
        .text_color(t.faint)
        .hover({
            let muted = t.muted;
            move |style| style.text_color(muted)
        })
        .child(label)
}

impl Root {
    fn prop(&self, id: &'static str, label: &'static str, t: &Theme) -> Stateful<Div> {
        let raised = t.raised;
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .relative()
            .flex()
            .items_center()
            .flex_grow(1.)
            .w(gpui::relative(0.5))
            .min_w(px(PROP_MIN_WIDTH))
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

    fn render_props(
        &self,
        row: &TaskRow,
        detail: Option<&Detail>,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Div {
        let anchors = self.anchors.clone();
        let value = || div().flex().items_center().gap(px(7.)).min_w_0();
        let plain = |text: String| value().child(div().truncate().child(text));
        let available = detail.map_or(String::new(), |detail| detail.available.clone());
        let project = detail.map_or(String::new(), |detail| detail.project_name.clone());
        div()
            .flex()
            .flex_wrap()
            .child(
                self.prop("status", "Status", t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.change_status(&ChangeStatus, window, cx)
                    }))
                    .child(anchor_probe("status", &anchors))
                    .child(
                        value()
                            .child(self.task_glyph(&row.id, row.status, t))
                            .child(status_name(row.status)),
                    ),
            )
            .child(
                self.prop("priority", "Priority", t)
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
                                t,
                            ))
                            .child(priority_name(row.priority)),
                    ),
            )
            .child(
                self.prop("available", "Available", t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_date_menu(DateField::Available, window, cx);
                    }))
                    .child(anchor_probe("available", &anchors))
                    .child(plain(available)),
            )
            .child(
                self.prop("due", "Due", t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_date_menu(DateField::Due, window, cx);
                    }))
                    .child(anchor_probe("due", &anchors))
                    .child(plain(if row.due.is_empty() {
                        "None".to_string()
                    } else {
                        row.due.clone()
                    })),
            )
            .child(
                self.prop("project", "Project", t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.change_project(&ChangeProject, window, cx)
                    }))
                    .child(anchor_probe("project", &anchors))
                    .child(
                        value()
                            .child(project_dot(&row.project_key, t))
                            .child(project),
                    ),
            )
            .child(
                self.prop("labels", "Labels", t)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.edit_labels(&EditLabels, window, cx)
                        }),
                    )
                    .child(anchor_probe("labels", &anchors))
                    .child(plain(if row.labels.is_empty() {
                        "None".to_string()
                    } else {
                        row.labels.join(", ")
                    })),
            )
            .when(row.recurring, |props| {
                let series = self
                    .session
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.series.clone());
                props.child(
                    self.prop("recurrence", "Recurrence", t)
                        .child(match series {
                            Some((id, reference)) => bare_btn("open-series", "Open series", t)
                                .text_color(t.body)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_series(id.clone(), window, cx);
                                }))
                                .child(format!("↻ {reference}"))
                                .into_any_element(),
                            None => plain("Repeats".to_string()).into_any_element(),
                        }),
                )
            })
    }

    fn render_title(&self, size: f32, window: &Window, cx: &Context<Self>, t: &Theme) -> Div {
        let focused = self.title.read(cx).focus.is_focused(window);
        let raised = t.raised;
        div()
            .mt(px(2.))
            .mb(px(12.))
            .ml(px(-4.))
            .px(px(4.))
            .py(px(2.))
            .rounded(px(5.))
            .text_size(px(size))
            .line_height(px(size * 1.3))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.ink)
            .when(focused, |field| {
                field.bg(raised).shadow(vec![t.ring(t.tint_line)])
            })
            .child(self.title.clone())
    }

    fn render_byline(&self, row: &TaskRow, cx: &mut Context<Self>, t: &Theme) -> Div {
        let byline = &row.byline;
        let when = if self.created_exact {
            &byline.created_on
        } else {
            &byline.created_ago
        };
        let source = byline
            .source
            .as_ref()
            .map_or_else(String::new, |source| format!(" from {source}"));
        let ink = t.ink;
        div()
            .flex()
            .flex_wrap()
            .gap_x(px(6.))
            .mt(px(-8.))
            .mb(px(12.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(t.muted)
            .child(
                div()
                    .id("byline-created")
                    .role(Role::Button)
                    .aria_label("Switch between relative and exact creation time")
                    .hover(move |style| style.text_color(ink))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.created_exact = !this.created_exact;
                        cx.notify();
                    }))
                    .child(format!("Created {when}{source}")),
            )
            .when_some(byline.updated_ago.clone(), |line, updated| {
                line.child(div().text_color(t.faint).child("·"))
                    .child(format!("Updated {updated}"))
            })
    }

    fn render_description(&self, row: &TaskRow, cx: &mut Context<Self>, t: &Theme) -> Div {
        let empty = self
            .session
            .detail
            .as_ref()
            .is_none_or(|detail| detail.description.is_empty());
        let head = section_head("Description", t);
        let Some(edit) = self.body_edit.as_ref().filter(|edit| edit.task == row.id) else {
            return div()
                .mt(px(20.))
                .child(
                    head.child(
                        side_action("description-edit", "Edit", t).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.edit_description(&EditDescription, window, cx)
                            },
                        )),
                    ),
                )
                .child(
                    div()
                        .id("description")
                        .text_color(t.body)
                        .line_height(px(20.15))
                        .min_h(px(20.))
                        .cursor_text()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.edit_description(&EditDescription, window, cx);
                        }))
                        .when(empty, |text| {
                            text.text_color(t.faint).child("Add a description")
                        })
                        .when(!empty, |text| text.child(self.description.clone())),
                );
        };
        let incoming = edit.incoming.is_some();
        div()
            .mt(px(20.))
            .child(head)
            .when(incoming, |section| {
                section.child(
                    div()
                        .mb(px(6.))
                        .px(px(10.))
                        .py(px(6.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(t.raised)
                        .text_color(t.warn)
                        .text_size(px(12.))
                        .child(div().flex_1().child("Changed elsewhere while you edit"))
                        .child(side_action("incoming-review", "Review", t).on_click(
                            cx.listener(|this, _, window, cx| this.review_incoming(window, cx)),
                        )),
                )
            })
            .child(
                div()
                    .key_context("BodyEditor")
                    .px(px(9.))
                    .py(px(7.))
                    .min_h(px(120.))
                    .rounded(px(6.))
                    .bg(t.page)
                    .shadow(vec![t.inset_ring(t.tint_line)])
                    .text_color(t.ink)
                    .line_height(px(20.15))
                    .child(self.body.clone()),
            )
            .child(
                small(12.)
                    .mt(px(6.))
                    .text_color(t.faint)
                    .child("⌘↵ or clicking away saves. Esc discards."),
            )
    }

    fn render_images(&self, detail: &Detail, cx: &mut Context<Self>, t: &Theme) -> Div {
        let mut thumbs = div().flex().flex_wrap().gap(px(6.));
        for (ix, image) in detail.images.iter().enumerate() {
            let shown = image.clone();
            let thumb = div()
                .id(("thumb", ix))
                .role(Role::Button)
                .aria_label(if image.alt.is_empty() {
                    image.name.clone()
                } else {
                    image.alt.clone()
                })
                .w(px(THUMB_WIDTH))
                .h(px(THUMB_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .rounded(px(6.))
                .bg(t.raised)
                .shadow(vec![t.inset_ring(t.line)])
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_image(shown.clone(), window, cx);
                }));
            thumbs = thumbs.child(match &image.preview {
                Some(path) => thumb.child(
                    img(path.clone())
                        .size_full()
                        .rounded(px(6.))
                        .object_fit(ObjectFit::Cover),
                ),
                None => thumb.child(
                    small(11.)
                        .text_center()
                        .text_color(t.faint)
                        .child("Not on this device"),
                ),
            });
        }
        div()
            .mt(px(20.))
            .child(
                section_head("Images", t).child(
                    side_action("image-add", "Add", t)
                        .on_click(cx.listener(|this, _, _, cx| this.add_image_from_disk(cx))),
                ),
            )
            .child(thumbs)
    }

    fn link_row(
        &self,
        link: &Link,
        id: gpui::ElementId,
        remove: Option<Command>,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        let target = link.id.clone();
        div()
            .id(id)
            .group(ROW_GROUP)
            .role(Role::Link)
            .aria_label(format!("{} {}", link.reference, link.title))
            .flex()
            .items_center()
            .gap(px(7.))
            .min_h(px(27.))
            .px(px(4.))
            .ml(px(-4.))
            .rounded(px(5.))
            .hover({
                let raised = t.raised;
                move |style| style.bg(raised)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.follow(target.clone(), window, cx);
            }))
            .child(status_glyph(link.status, t))
            .child(ref_label(&link.reference, &link.project_key, None, t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.body)
                    .child(link.title.clone()),
            )
            .when_some(remove, |line, command| {
                line.child(
                    row_action(
                        gpui::ElementId::Name(format!("remove-{}", link.id).into()),
                        "Remove",
                        t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.session.write(command.clone());
                        cx.notify();
                    })),
                )
            })
    }

    fn render_links(
        &self,
        id: &TaskId,
        detail: &Detail,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<Div> {
        let mut sections = Vec::new();
        if let Some(epic) = &detail.epic {
            let closed = epic.status.is_terminal();
            let head = if closed { "Epic closed" } else { "Epic" };
            sections.push(
                div()
                    .mt(px(20.))
                    .child(section_head(head, t))
                    .child(self.link_row(epic, "epic-link".into(), None, cx, t)),
            );
        }
        if !detail.blocked_by.is_empty() {
            let mut section = div().mt(px(20.)).child(section_head("Blocked by", t));
            for (ix, link) in detail.blocked_by.iter().enumerate() {
                let remove = Command::RemoveBlocker(id.clone(), link.id.clone());
                section = section.child(self.link_row(
                    link,
                    ("blocked-by", ix).into(),
                    Some(remove),
                    cx,
                    t,
                ));
            }
            sections.push(section);
        }
        if !detail.blocks.is_empty() {
            let mut section = div().mt(px(20.)).child(section_head("Blocks", t));
            for (ix, link) in detail.blocks.iter().enumerate() {
                section = section.child(self.link_row(link, ("blocks", ix).into(), None, cx, t));
            }
            sections.push(section);
        }
        if !detail.related.is_empty() {
            let mut section = div().mt(px(20.)).child(section_head("Related", t));
            for (ix, link) in detail.related.iter().enumerate() {
                let remove = Command::RemoveRelated(id.clone(), link.id.clone());
                section =
                    section.child(self.link_row(link, ("related", ix).into(), Some(remove), cx, t));
            }
            sections.push(section);
        }
        sections
    }

    fn render_metadata(
        &self,
        id: &TaskId,
        detail: &Detail,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Div {
        let mut section = div().mt(px(20.)).child(section_head("Metadata", t));
        for (ix, (key, value)) in detail.metadata.iter().enumerate() {
            let (field, removed) = (key.clone(), key.clone());
            let task = id.clone();
            section =
                section.child(
                    div()
                        .id(("metadata", ix))
                        .role(Role::Button)
                        .aria_label(format!("Edit {key}"))
                        .group(ROW_GROUP)
                        .flex()
                        .items_center()
                        .min_h(px(27.))
                        .px(px(4.))
                        .ml(px(-4.))
                        .rounded(px(5.))
                        .hover({
                            let raised = t.raised;
                            move |style| style.bg(raised)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_meta(field.clone(), window, cx);
                        }))
                        .child(
                            div()
                                .flex_none()
                                .w(px(META_KEY_WIDTH))
                                .truncate()
                                .text_color(t.faint)
                                .child(key.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(if value.is_empty() { t.faint } else { t.body })
                                .child(if value.is_empty() {
                                    "Empty value".to_string()
                                } else {
                                    value.clone()
                                }),
                        )
                        .child(row_action(("metadata-remove", ix), "Remove", t).on_click(
                            cx.listener(move |this, _, _, cx| {
                                let command =
                                    Command::SetMetadata(task.clone(), removed.clone(), None);
                                this.session.write(command);
                                cx.notify();
                            }),
                        )),
                );
        }
        section
    }

    fn render_notes(&self, id: &TaskId, window: &Window, cx: &mut Context<Self>, t: &Theme) -> Div {
        let note_focused = self.note.read(cx).focus.is_focused(window);
        let mut notes = div().mt(px(20.)).child(section_head("Notes", t));
        for (ix, (note, view)) in self.note_views.iter().enumerate() {
            let (task, note_id) = (id.clone(), note.id.clone());
            notes = notes.child(
                div()
                    .group(ROW_GROUP)
                    .mb(px(10.))
                    .text_color(t.body)
                    .line_height(px(19.5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                small(11.)
                                    .line_height(px(15.4))
                                    .text_color(t.faint)
                                    .child(note.at.clone()),
                            )
                            .child(row_action(("note-delete", ix), "Delete", t).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let command =
                                        Command::DeleteNote(task.clone(), note_id.clone());
                                    this.session.write(command);
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(view.clone()),
            );
        }
        notes.child(
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
        )
    }

    fn render_activity(&self, row: &TaskRow, t: &Theme) -> Div {
        let mut section = div().mt(px(20.)).child(section_head("Activity", t));
        for (text, at) in &row.activity {
            section = section.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .py(px(3.))
                    .text_size(px(12.))
                    .line_height(px(ACTIVITY_LINE_HEIGHT))
                    .child(
                        div()
                            .flex_none()
                            .mt(px((ACTIVITY_LINE_HEIGHT - ACTIVITY_DOT) / 2.))
                            .size(px(ACTIVITY_DOT))
                            .rounded_full()
                            .bg(t.faint),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(t.body)
                            .child(text.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_color(t.faint)
                            .child(at.clone()),
                    ),
            );
        }
        section
    }

    fn deleted_banner(&self, cx: &mut Context<Self>, t: &Theme) -> Div {
        div()
            .mb(px(12.))
            .px(px(10.))
            .py(px(6.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(t.raised)
            .text_color(t.warn)
            .child(div().flex_1().child("This task is deleted"))
            .child(side_action("banner-restore", "Restore", t).on_click(
                cx.listener(|this, _, window, cx| this.restore_task(&RestoreTask, window, cx)),
            ))
    }

    /// The task's sections below its description.
    fn render_sections(
        &self,
        row: &TaskRow,
        window: &Window,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let mut sections = vec![self.render_description(row, cx, t).into_any_element()];
        if let Some(detail) = self.session.detail.clone() {
            if row.is_epic {
                sections.push(
                    self.render_epic_section(row, &detail, cx, t)
                        .into_any_element(),
                );
            }
            if !detail.images.is_empty() {
                sections.push(self.render_images(&detail, cx, t).into_any_element());
            }
            sections.extend(
                self.render_links(&row.id, &detail, cx, t)
                    .into_iter()
                    .map(|section| section.into_any_element()),
            );
            if !detail.metadata.is_empty() {
                sections.push(
                    self.render_metadata(&row.id, &detail, cx, t)
                        .into_any_element(),
                );
            }
            sections.push(self.render_notes(&row.id, window, cx, t).into_any_element());
        }
        if !row.activity.is_empty() {
            sections.push(self.render_activity(row, t).into_any_element());
        }
        sections
    }

    fn top_actions(&self, row: &TaskRow, cx: &mut Context<Self>, t: &Theme) -> Div {
        let done = row.status == TaskStatus::Done;
        let closed = done || row.status == TaskStatus::Canceled;
        let start = !closed && row.status != TaskStatus::Active && !row.is_epic;
        let id = row.id.clone();
        let done_label = match (done, row.is_epic) {
            (true, _) => "Reopen",
            (false, true) => "Close epic",
            (false, false) => "Mark done",
        };
        let expanded = self.session.expanded;
        let anchors = self.anchors.clone();
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(start, |top| {
                top.child(
                    btn("start", "Start", t)
                        .px(px(8.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.session.set_status(&id, TaskStatus::Active);
                            this.after_status_motion(cx);
                        }))
                        .child("Start"),
                )
            })
            .child(
                btn("mark-done", done_label, t)
                    .px(px(8.))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.mark_done(&MarkDone, window, cx)),
                    )
                    .child(done_label),
            )
            .when(!expanded, |top| {
                top.child(
                    btn("expand", "Expand", t)
                        .px(px(8.))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.expand(&Expand, window, cx);
                        }))
                        .child("Expand"),
                )
            })
            .child(
                btn("add", "Add", t)
                    .relative()
                    .px(px(8.))
                    .child(anchor_probe("add", &anchors))
                    .on_click(cx.listener(|this, _, window, cx| this.add_menu(window, cx)))
                    .child("Add…"),
            )
            .child(
                btn("overflow", "More actions", t)
                    .relative()
                    .px(px(8.))
                    .child(anchor_probe("overflow", &anchors))
                    .on_click(cx.listener(|this, _, window, cx| this.overflow_menu(window, cx)))
                    .child("⋯"),
            )
    }

    fn trail_button(&self, cx: &mut Context<Self>, t: &Theme) -> Option<impl IntoElement + use<>> {
        let label = self.session.trail.last()?.label.clone();
        Some(
            bare_btn("trail-back", format!("Back to {label}"), t)
                .max_w(px(150.))
                .px(px(6.))
                .text_color(t.muted)
                .on_click(cx.listener(|this, _, window, cx| this.trail_back(window, cx)))
                .child("←")
                .child(div().truncate().child(label)),
        )
    }

    pub(crate) fn render_detail(
        &self,
        width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let panel = div()
            .flex_none()
            .w(px(width))
            .h_full()
            .ml(px(-SPLIT_OVERLAP))
            .flex()
            .flex_col()
            .bg(t.page)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach_paths(paths.paths(), cx);
            }))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(px(1.))
                    .bg(t.line),
            );
        let Some(row) = self.session.target_row().cloned() else {
            return panel;
        };
        let detail = self.session.detail.clone();
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
            .children(self.trail_button(cx, &t))
            .child(
                div()
                    .id("detail-ref")
                    .role(Role::Button)
                    .aria_label("Copy ref")
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this, _, window, cx| this.copy_ref(&CopyRef, window, cx)),
                    )
                    .child(ref_label(&row.reference, &row.project_key, None, &t)),
            )
            .children(self.epic_ring(&row, &t))
            .child(div().flex_1())
            .child(self.top_actions(&row, cx, &t));
        let body = div()
            .id("detail-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.detail_scroll)
            .pt(px(12.))
            .px(px(18.))
            .pb(px(40.))
            .children(
                detail
                    .as_ref()
                    .and_then(|detail| self.conflict_banner(detail, cx, &t)),
            )
            .when(row.deleted, |body| body.child(self.deleted_banner(cx, &t)))
            .child(self.render_title(19., window, cx, &t))
            .child(self.render_byline(&row, cx, &t))
            .child(self.render_props(&row, detail.as_ref(), cx, &t))
            .children(self.render_sections(&row, window, cx, &t));
        panel.child(top).child(body)
    }

    /// The target filling the window, with its fields beside its content.
    pub(crate) fn render_doc(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let doc = div().flex_1().min_w_0().h_full().on_drop(cx.listener(
            |this, paths: &ExternalPaths, _, cx| {
                this.attach_paths(paths.paths(), cx);
            },
        ));
        let Some(row) = self.session.target_row().cloned() else {
            return doc;
        };
        let detail = self.session.detail.clone();
        doc.child(
            div()
                .id("doc-body")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.detail_scroll)
                .pt(px(26.))
                .px(px(24.))
                .pb(px(60.))
                .child(
                    div()
                        .mx_auto()
                        .max_w(px(DOC_WIDTH))
                        .flex()
                        .gap(px(44.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(row.deleted, |body| body.child(self.deleted_banner(cx, &t)))
                                .child(
                                    div()
                                        .mb(px(6.))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .children(self.trail_button(cx, &t))
                                        .child(ref_label(
                                            &row.reference,
                                            &row.project_key,
                                            None,
                                            &t,
                                        )),
                                )
                                .child(self.render_title(26., window, cx, &t))
                                .child(self.render_byline(&row, cx, &t))
                                .child(self.top_actions(&row, cx, &t).ml(px(-8.)))
                                .children(self.render_sections(&row, window, cx, &t)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(px(DOC_SIDE_WIDTH))
                                .child(self.render_props(&row, detail.as_ref(), cx, &t)),
                        ),
                ),
        )
    }

    pub(crate) fn edit_description(
        &mut self,
        _: &EditDescription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(id), Some(detail)) = (self.session.target().cloned(), &self.session.detail)
        else {
            return;
        };
        if !self.session.expanded {
            self.details_open = true;
        }
        let base = detail.description.clone();
        self.body
            .update(cx, |editor, cx| editor.set_text(&base, cx));
        self.body_edit = Some(BodyEdit {
            task: id,
            base,
            incoming: None,
        });
        let focus = self.body.read(cx).focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Saves the description being written, if it says something new.
    pub(crate) fn commit_description(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.body_edit.take() else {
            return;
        };
        let text = self.body.read(cx).text().to_string();
        if text != edit.base {
            self.session.write(Command::SetDescription(edit.task, text));
        }
        cx.notify();
    }

    pub(crate) fn save_description(
        &mut self,
        _: &SaveDescription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_description(cx);
        window.focus(&self.focus, cx);
    }

    /// Leaves the editor without saving.
    pub(crate) fn cancel_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.body_edit = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Leaves the editor after its text was saved another way.
    pub(crate) fn finish_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_description(window, cx);
    }

    fn review_incoming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.body_edit.as_ref() else {
            return;
        };
        let Some(theirs) = edit.incoming.clone() else {
            return;
        };
        let mine = self.body.read(cx).text().to_string();
        let task = edit.task.clone();
        self.open_incoming(task, mine, theirs, window, cx);
    }

    pub(crate) fn on_body_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Cancel => self.cancel_description(window, cx),
            // A description that changed elsewhere waits for a decision.
            EditorEvent::Blur => {
                let waiting = self
                    .body_edit
                    .as_ref()
                    .is_some_and(|edit| edit.incoming.is_some());
                if !waiting {
                    self.commit_description(cx);
                }
            }
            EditorEvent::PastedImage(pasted) => {
                if let (Some(id), Some(image)) =
                    (self.session.target().cloned(), pasted_image(pasted.clone()))
                {
                    self.session.write(Command::AddImage(id, image));
                    cx.notify();
                }
            }
            _ => {}
        }
    }
}

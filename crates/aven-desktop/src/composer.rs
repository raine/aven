//! Writing a new task: a title with inline tokens, chips for its fields, and
//! details behind one toggle. A draft survives closing until it is discarded.
use std::rc::Rc;
use std::time::Duration;

use accesskit::Role;
use aven_core::choices::TaskStatus;
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Entity, ExternalPaths, FocusHandle, KeyDownEvent,
    MouseButton, MouseDownEvent, Subscription, Window, div, prelude::*,
};

use crate::actions::*;
use crate::commands::{pasted_image, read_image};
use crate::draft::{Draft, DraftProblem, schedule_preview, token_preview};
use crate::input::{Editor, EditorEvent, Mode};
use crate::kit::*;
use crate::overlays::{Entry, Mark, MenuOptions};
use crate::shell::Root;
use crate::theme::{Theme, theme};

const DIALOG_CLOSE: Duration = Duration::from_millis(120);
const COMPOSER_WIDTH: f32 = 560.;
const COMPOSER_MORE_WIDTH: f32 = 760.;
const QUICK_SCHEDULES: [(&str, &str); 4] = [
    ("Tomorrow", "tomorrow"),
    ("Next Monday", "next monday"),
    ("Due Friday", "due friday"),
    ("Every Friday", "every friday at 09:00"),
];

pub struct Composer {
    /// The fields that have no editor. Text lives in the editors.
    draft: Draft,
    title: Entity<Editor>,
    schedule: Entity<Editor>,
    body: Entity<Editor>,
    /// Where a refusal about the status sends focus.
    status: FocusHandle,
    /// Where a refusal about being an epic sends focus.
    epic: FocusHandle,
    prior: Option<FocusHandle>,
    run: usize,
    pub closing: bool,
    reading: bool,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    /// The whole draft as it stands.
    fn current(&self, cx: &gpui::App) -> Draft {
        Draft {
            title: self.title.read(cx).text().to_string(),
            schedule: self.schedule.read(cx).text().to_string(),
            body: self.body.read(cx).text().to_string(),
            ..self.draft.clone()
        }
    }

    fn show(&mut self, draft: Draft, cx: &mut Context<Root>) {
        self.title
            .update(cx, |editor, cx| editor.set_text(&draft.title, cx));
        self.schedule
            .update(cx, |editor, cx| editor.set_text(&draft.schedule, cx));
        self.body
            .update(cx, |editor, cx| editor.set_text(&draft.body, cx));
        self.draft = draft;
    }
}

impl Root {
    pub(crate) fn composer_open(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| !composer.closing)
    }

    pub(crate) fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.open_composer(window, cx);
    }

    pub(crate) fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer_open() {
            return;
        }
        let draft = std::mem::take(&mut self.kept_draft);
        let title = cx.new(|cx| {
            let placeholder = "What needs doing? #label !high";
            Editor::new(Mode::Single, placeholder, window, cx)
        });
        let schedule = cx.new(|cx| {
            let placeholder = "tomorrow, due next friday, every friday at 09:00";
            Editor::new(Mode::Single, placeholder, window, cx)
        });
        let body = cx.new(|cx| {
            let placeholder = "Markdown works. Paste an image to attach it.";
            Editor::new(Mode::Multi, placeholder, window, cx)
                .enter_breaks()
                .takes_images()
                .max_height(px(220.))
        });
        let subscriptions = [&title, &schedule, &body]
            .map(|editor| cx.subscribe_in(editor, window, Self::on_composer_event))
            .into_iter()
            .collect();
        let focus = title.read(cx).focus.clone();
        let mut composer = Composer {
            draft: Draft::default(),
            title,
            schedule,
            body,
            status: cx.focus_handle(),
            epic: cx.focus_handle(),
            prior: window.focused(cx),
            run: self.session.motion.next_run(),
            closing: false,
            reading: false,
            _subscriptions: subscriptions,
        };
        composer.show(draft, cx);
        self.composer = Some(composer);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Fades the composer out. Pointer input passes through while it does.
    fn close_composer(&mut self, keep: Option<Draft>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composer) = self.composer.as_mut().filter(|composer| !composer.closing) else {
            return;
        };
        composer.closing = true;
        let run = composer.run;
        let focus = composer.prior.clone().unwrap_or_else(|| self.focus.clone());
        self.kept_draft = keep.unwrap_or_default();
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

    /// Closes the composer with its draft kept for next time.
    pub(crate) fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            self.close_dialog(window, cx);
        } else if let Some(draft) = self.composer_draft(cx) {
            self.close_composer(Some(draft), window, cx);
        }
    }

    pub(crate) fn composer_draft(&self, cx: &gpui::App) -> Option<Draft> {
        let composer = self
            .composer
            .as_ref()
            .filter(|composer| !composer.closing)?;
        Some(composer.current(cx))
    }

    fn discard_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.composer_draft(cx) else {
            return;
        };
        if draft.has_text() {
            self.discarded = Some(draft);
            self.session.show_toast(
                "Draft discarded".to_string(),
                Some(crate::session::ToastAction::RestoreDraft),
            );
            self.arm_toast(cx);
        }
        self.close_composer(None, window, cx);
    }

    pub(crate) fn restore_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.discarded.take() {
            self.kept_draft = draft;
            self.open_composer(window, cx);
        }
    }

    /// A create that failed comes back here whatever is being written.
    pub(crate) fn return_failed_draft(
        &mut self,
        draft: Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.kept_draft = draft;
        self.open_composer(window, cx);
    }

    fn submit_composer(&mut self, another: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.composer_draft(cx) else {
            return;
        };
        match draft.task() {
            Ok(mut task) => {
                // Left to inference, a task belongs to the project in scope.
                task.project = task.project.or(self.session.snapshot.view.project.clone());
                self.session.create(task);
                if another {
                    if let Some(composer) = self.composer.as_mut() {
                        let next = draft.after_created();
                        let focus = composer.title.read(cx).focus.clone();
                        composer.show(next, cx);
                        window.focus(&focus, cx);
                    }
                    cx.notify();
                } else {
                    self.close_composer(None, window, cx);
                }
            }
            Err(problem @ DraftProblem::Title) => self.refuse_draft(problem, None, window, cx),
            Err(problem @ DraftProblem::RecurringAttachments) => {
                self.update_draft(cx, |draft| draft.more = true);
                self.refuse_draft(problem, None, window, cx);
            }
            Err(problem @ DraftProblem::RecurringEpic) => {
                let epic = self.composer.as_ref().map(|composer| composer.epic.clone());
                self.refuse_draft(problem, epic, window, cx);
            }
            Err(problem @ DraftProblem::RecurringStatus) => {
                let status = self
                    .composer
                    .as_ref()
                    .map(|composer| composer.status.clone());
                self.refuse_draft(problem, status, window, cx);
            }
            Err(DraftProblem::Schedule) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.draft.more = true;
                    let focus = composer.schedule.read(cx).focus.clone();
                    window.focus(&focus, cx);
                }
                cx.notify();
            }
        }
    }

    fn refuse_draft(
        &mut self,
        problem: DraftProblem,
        focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session.show_toast(problem.message().to_string(), None);
        self.arm_toast(cx);
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(crate) fn create_task(
        &mut self,
        _: &CreateTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_composer(false, window, cx);
    }

    pub(crate) fn create_another(
        &mut self,
        _: &CreateAnother,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repeats = self.composer_draft(cx).is_some_and(|draft| draft.repeats());
        if !repeats {
            self.submit_composer(true, window, cx);
        }
    }

    pub(crate) fn read_notes(&mut self, _: &ReadNotes, _: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.composer_draft(cx) else {
            return;
        };
        let notes = draft.intake_text();
        if notes.is_empty() {
            self.session
                .show_toast("Add rough notes first".to_string(), None);
            self.arm_toast(cx);
        } else {
            self.session.intake(notes, draft.project.clone());
            if let Some(composer) = self.composer.as_mut() {
                composer.reading = true;
            }
        }
        cx.notify();
    }

    /// Takes what the reader made of the notes.
    pub(crate) fn read_notes_back(
        &mut self,
        task: Option<crate::adapter::NewTask>,
        cx: &mut Context<Self>,
    ) {
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        composer.reading = false;
        if let Some(task) = task {
            let mut draft = composer.current(cx);
            draft.fill(task);
            composer.show(draft, cx);
        }
        cx.notify();
    }

    pub(crate) fn composer_fields(&self, cx: &Context<Self>) -> Vec<FocusHandle> {
        let Some(composer) = self.composer.as_ref() else {
            return Vec::new();
        };
        let mut fields = vec![composer.title.read(cx).focus.clone()];
        if composer.draft.more {
            fields.push(composer.schedule.read(cx).focus.clone());
            fields.push(composer.body.read(cx).focus.clone());
        }
        fields
    }

    fn on_composer_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Enter in the description breaks the line, so only one-line fields
            // reach here.
            EditorEvent::Submit => self.submit_composer(false, window, cx),
            EditorEvent::Cancel => self.dismiss(&Dismiss, window, cx),
            EditorEvent::Changed => cx.notify(),
            EditorEvent::PastedImage(pasted) => {
                if let (Some(image), Some(composer)) =
                    (pasted_image(pasted.clone()), self.composer.as_mut())
                {
                    composer.draft.add_image(image);
                    composer.draft.more = true;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Draft)) {
        if let Some(composer) = self.composer.as_mut() {
            change(&mut composer.draft);
        }
        cx.notify();
    }

    fn pick_composer_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut entries = vec![Entry::new("Infer", |root, _, cx| {
            root.update_draft(cx, |draft| draft.project = None);
        })];
        for project in &self.session.snapshot.projects {
            let key = project.key.clone();
            entries.push(
                Entry::new(project.name.clone(), move |root, _, cx| {
                    let key = key.clone();
                    root.update_draft(cx, |draft| draft.project = Some(key));
                })
                .hint(project.prefix.clone())
                .mark(Mark::Project(project.prefix.clone())),
            );
        }
        let anchor = self.anchor("c-project");
        self.open_menu(
            entries,
            "Project",
            anchor,
            MenuOptions::default(),
            window,
            cx,
        );
    }

    fn toggle_composer_epic(&mut self, cx: &mut Context<Self>) {
        self.update_draft(cx, |draft| {
            draft.is_epic = !draft.is_epic;
            if draft.is_epic && draft.status == Some(TaskStatus::Inbox) {
                draft.status = None;
            }
        });
    }

    fn pick_composer_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_epic = self.composer_draft(cx).is_some_and(|draft| draft.is_epic);
        let mut entries = vec![Entry::new("Auto", |root, _, cx| {
            root.update_draft(cx, |draft| draft.status = None);
        })];
        for (status, name) in STATUSES
            .iter()
            .filter(|(status, _)| !is_epic || *status != TaskStatus::Inbox)
        {
            let status = *status;
            entries.push(
                Entry::new(*name, move |root, _, cx| {
                    root.update_draft(cx, |draft| draft.status = Some(status));
                })
                .mark(Mark::Status(status)),
            );
        }
        let anchor = self.anchor("c-status");
        self.open_menu(
            entries,
            "Status",
            anchor,
            MenuOptions::default(),
            window,
            cx,
        );
    }

    fn pick_composer_priority(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = PRIORITIES
            .iter()
            .map(|(priority, name)| {
                let priority = *priority;
                Entry::new(*name, move |root, _, cx| {
                    root.update_draft(cx, |draft| draft.priority = priority);
                })
                .mark(Mark::Priority(priority))
            })
            .collect();
        let anchor = self.anchor("c-priority");
        self.open_menu(
            entries,
            "Priority",
            anchor,
            MenuOptions::default(),
            window,
            cx,
        );
    }

    fn pick_composer_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let held = self
            .composer_draft(cx)
            .map(|draft| draft.labels)
            .unwrap_or_default();
        let save: Rc<dyn Fn(&mut Root, &mut Context<Root>)> = Rc::new(|root, cx| {
            let labels = root.menu_checked();
            root.update_draft(cx, |draft| draft.labels = labels);
        });
        let entry = move |label: String, save: Rc<dyn Fn(&mut Root, &mut Context<Root>)>| {
            Entry::new(label, move |root, _, cx| save(root, cx))
        };
        let mut labels = self.session.snapshot.labels.clone();
        labels.extend(held.iter().cloned());
        labels.sort();
        labels.dedup();
        let entries = labels
            .into_iter()
            .map(|label| {
                let checked = held.contains(&label);
                entry(label, save.clone()).checked(checked)
            })
            .collect();
        let create = Rc::new(move |label: String| entry(label, save.clone()));
        let options = MenuOptions {
            multi: true,
            create: Some(create),
            ..MenuOptions::default()
        };
        let anchor = self.anchor("c-labels");
        self.open_menu(entries, "Toggle labels", anchor, options, window, cx);
    }

    fn pick_composer_metadata(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let held = self
            .composer_draft(cx)
            .map(|draft| draft.metadata)
            .unwrap_or_default();
        let entries = self
            .session
            .snapshot
            .fields
            .iter()
            .map(|field| {
                let key = field.clone();
                let on = held.iter().any(|(held, _)| held == field);
                Entry::new(field.clone(), move |root, window, cx| {
                    root.prompt_composer_metadata(key.clone(), window, cx);
                })
                .checked(on)
            })
            .collect();
        let create = Rc::new(|label: String| {
            let key = label.clone();
            Entry::new(label, move |root, window, cx| {
                root.prompt_composer_metadata(key.clone(), window, cx);
            })
        });
        let options = MenuOptions {
            create: Some(create),
            ..MenuOptions::default()
        };
        let anchor = self.anchor("c-metadata");
        self.open_menu(entries, "Metadata field", anchor, options, window, cx);
    }

    fn prompt_composer_metadata(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = self
            .composer_draft(cx)
            .and_then(|draft| {
                let held = draft.metadata.iter();
                held.clone()
                    .find(|(field, _)| *field == key)
                    .map(|(_, value)| value.clone())
            })
            .unwrap_or_default();
        let field = key.clone();
        let save = Rc::new(
            move |root: &mut Root, text: String, _: &mut Window, cx: &mut Context<Root>| {
                let field = field.clone();
                root.update_draft(cx, |draft| {
                    draft.metadata.retain(|(held, _)| *held != field);
                    draft.metadata.push((field, text));
                });
            },
        );
        let hint = "An empty value is different from no value.";
        self.open_prompt(
            format!("Set {key}"),
            "Value",
            hint,
            &value,
            save,
            window,
            cx,
        );
    }

    fn drop_on_composer(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let images: Vec<_> = paths.iter().filter_map(|path| read_image(path)).collect();
        self.update_draft(cx, |draft| {
            draft.more |= !images.is_empty();
            for image in images {
                draft.add_image(image);
            }
        });
    }

    fn chip(
        id: &'static str,
        name: String,
        leading: Option<AnyElement>,
        anchors: &Anchors,
        t: &Theme,
    ) -> gpui::Stateful<gpui::Div> {
        btn(id, name.clone(), t)
            .relative()
            .h(px(26.))
            .px(px(8.))
            .text_size(px(12.))
            .shadow(vec![t.inset_ring(t.line)])
            .child(anchor_probe(id, anchors))
            .children(leading)
            .child(name)
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
        let draft = composer.current(cx);
        let parts = crate::draft::parse_title(&draft.title);
        let priority = parts.priority.unwrap_or(draft.priority);
        let project_name = draft
            .project
            .as_ref()
            .and_then(|key| {
                let projects = &self.session.snapshot.projects;
                projects.iter().find(|project| project.key == *key)
            })
            .map(|project| project.name.clone())
            .or_else(|| {
                let scope = self.session.snapshot.view.project.as_ref()?;
                let projects = &self.session.snapshot.projects;
                let project = projects.iter().find(|project| project.key == *scope)?;
                Some(format!("{} (inferred)", project.name))
            })
            .unwrap_or_else(|| "Project (inferred)".to_string());
        let status = draft.shown_status();
        let status_name_shown = if draft.status.is_some() {
            status_name(status).to_string()
        } else {
            format!("{} (auto)", status_name(status))
        };
        let labels = parts.labels.iter().chain(&draft.labels).count();
        let anchors = &self.anchors;
        let preview = token_preview(&draft.title);
        let schedule = schedule_preview(&draft.schedule);
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(5.))
            .px(px(14.))
            .pb(px(10.))
            .child(
                Self::chip("c-project", project_name, None, anchors, &t).on_click(
                    cx.listener(|this, _, window, cx| this.pick_composer_project(window, cx)),
                ),
            )
            .child(
                Self::chip(
                    "c-status",
                    status_name_shown,
                    Some(status_glyph(status, &t).into_any_element()),
                    anchors,
                    &t,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick_composer_status(window, cx)))
                .track_focus(&composer.status)
                .when(composer.status.is_focused(window), |chip| {
                    chip.shadow(vec![t.ring(t.accent)])
                })
                .on_key_down(cx.listener(
                    |this, event: &KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            this.pick_composer_status(window, cx);
                        }
                    },
                )),
            )
            .child(
                Self::chip(
                    "c-priority",
                    priority_name(priority).to_string(),
                    Some(priority_icon(priority, &t).into_any_element()),
                    anchors,
                    &t,
                )
                .on_click(
                    cx.listener(|this, _, window, cx| this.pick_composer_priority(window, cx)),
                ),
            )
            .child(
                Self::chip(
                    "c-labels",
                    if labels == 0 {
                        "Labels".to_string()
                    } else {
                        format!("Labels {labels}")
                    },
                    None,
                    anchors,
                    &t,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick_composer_labels(window, cx))),
            )
            .when(draft.more || !draft.metadata.is_empty(), |row| {
                row.child(
                    Self::chip(
                        "c-metadata",
                        if draft.metadata.is_empty() {
                            "Metadata".to_string()
                        } else {
                            format!("Metadata {}", draft.metadata.len())
                        },
                        None,
                        anchors,
                        &t,
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.pick_composer_metadata(window, cx)),
                    ),
                )
            })
            .when(draft.more || draft.is_epic, |row| {
                row.child(
                    Self::chip("c-epic", "Epic".to_string(), None, anchors, &t)
                        .when(draft.is_epic, |chip| {
                            chip.text_color(t.accent)
                                .shadow(vec![t.inset_ring(t.tint_line)])
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_composer_epic(cx)))
                        .track_focus(&composer.epic)
                        .when(composer.epic.is_focused(window), |chip| {
                            chip.shadow(vec![t.ring(t.accent)])
                        })
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.toggle_composer_epic(cx);
                            }
                        })),
                )
            })
            .child(
                btn(
                    "c-more",
                    if draft.more {
                        "Fewer details"
                    } else {
                        "More details"
                    },
                    &t,
                )
                .ml_auto()
                .h(px(26.))
                .px(px(8.))
                .text_size(px(12.))
                .text_color(t.muted)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.update_draft(cx, |draft| draft.more = !draft.more);
                }))
                .child(if draft.more {
                    "Fewer details"
                } else {
                    "More details"
                }),
            );

        let mut images = div().flex().flex_wrap().gap(px(5.)).mt(px(6.));
        for (ix, image) in draft.images.iter().enumerate() {
            images = images.child(
                btn(("c-image", ix), format!("Remove {}", image.name), &t)
                    .h(px(24.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .shadow(vec![t.inset_ring(t.line)])
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_draft(cx, |draft| {
                            draft.images.remove(ix);
                        });
                    }))
                    .child(image.name.clone())
                    .child(div().text_color(t.faint).child("×")),
            );
        }
        let more = div()
            .px(px(14.))
            .pb(px(12.))
            .child(small(12.).mb(px(4.)).text_color(t.faint).child("Schedule"))
            .child(
                div()
                    .h(px(30.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .shadow(vec![t.inset_ring(t.line)])
                    .text_color(t.ink)
                    .child(composer.schedule.clone()),
            )
            .child(
                self.quick_row(
                    "c-quick",
                    &composer.schedule,
                    QUICK_SCHEDULES
                        .map(|(label, value)| (label, value.to_string()))
                        .to_vec(),
                    cx,
                    &t,
                ),
            )
            .when(!draft.schedule.trim().is_empty(), |more| {
                more.child(
                    div()
                        .mt(px(6.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(7.))
                        .bg(t.raised)
                        .text_color(if schedule.valid { t.body } else { t.bad })
                        .child(schedule.text.clone()),
                )
            })
            .child(
                small(12.)
                    .mt(px(10.))
                    .mb(px(4.))
                    .text_color(t.faint)
                    .child("Description"),
            )
            .child(
                div()
                    .min_h(px(96.))
                    .px(px(9.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .shadow(vec![t.inset_ring(t.line)])
                    .text_color(t.ink)
                    .child(composer.body.clone()),
            )
            .when(!draft.images.is_empty(), |more| more.child(images))
            .child(
                div()
                    .mt(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        line_btn("c-add-image", "Add image", &t).on_click(cx.listener(
                            |this, _, _, cx| this.prompt_for_images(cx, Self::drop_on_composer),
                        )),
                    )
                    .child(
                        small(12.)
                            .text_color(t.faint)
                            .child("Or paste or drop one here"),
                    ),
            );

        let repeats = schedule.repeats;
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .py(px(10.))
            .border_t_1()
            .border_color(t.line)
            .child(
                btn(
                    "c-read",
                    if composer.reading {
                        "Reading…"
                    } else {
                        "Create with AI"
                    },
                    &t,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.read_notes(&ReadNotes, window, cx);
                }))
                .child(if composer.reading {
                    "Reading…"
                } else {
                    "Create with AI"
                })
                .child(kbd("⌃N", t.faint)),
            )
            .child(div().flex_1())
            .child(
                btn("c-discard", "Discard", &t)
                    .on_click(cx.listener(|this, _, window, cx| this.discard_draft(window, cx)))
                    .child("Discard"),
            )
            .child(
                btn("c-another", "Add another", &t)
                    .shadow(vec![t.inset_ring(t.line)])
                    .when(repeats, |button| button.opacity(0.5))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.create_another(&CreateAnother, window, cx);
                    }))
                    .child("Add another")
                    .child(kbd("⌃G", t.faint)),
            )
            .child(
                fill_btn("c-create", "Create", &t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.create_task(&CreateTask, window, cx);
                    }))
                    .child(kbd("⌘↵", t.on_fill)),
            );
        let width = if draft.more {
            COMPOSER_MORE_WIDTH
        } else {
            COMPOSER_WIDTH
        }
        .min(f32::from(viewport.width) - 32.);
        let panel = div()
            .id("composer")
            .role(Role::Dialog)
            .aria_label("New task")
            .key_context("Dialog Composer")
            .on_action(cx.listener(Self::create_task))
            .on_action(cx.listener(Self::create_another))
            .on_action(cx.listener(Self::read_notes))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::next_field))
            .on_action(cx.listener(Self::previous_field))
            .absolute()
            .left((viewport.width - px(width)) / 2.)
            .top(viewport.height * 0.11)
            .w(px(width))
            .max_h(viewport.height * 0.78)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(t.pop)
            .shadow(t.dlg_shadow())
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.drop_on_composer(paths.paths(), cx);
            }))
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
                    .child(composer.title.clone()),
            )
            .when(!preview.is_empty(), |panel| {
                panel.child(
                    small(12.)
                        .px(px(16.))
                        .pb(px(8.))
                        .text_color(t.faint)
                        .child(preview),
                )
            })
            .child(chips)
            .when(draft.more, |panel| panel.child(more))
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
                        this.dismiss(&Dismiss, window, cx);
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
}

//! The window's task-writing and popup boundary.
use gpui::{App, Context, Entity, Window};

use crate::actions::{Dismiss, NewTask};
use crate::adapter::NewTask as Task;
use crate::composer::{Composer, ComposerEvent, Source};
use crate::draft::Draft;
use crate::shell::Root;

impl Source {
    pub(crate) fn from_session(session: &crate::session::Session) -> Self {
        Self {
            projects: session.snapshot.projects.clone(),
            labels: session.snapshot.labels.clone(),
            fields: session.snapshot.fields.clone(),
            project: session
                .new_task_project()
                .map(|(key, why)| (key.to_string(), why)),
            first_project: session.first_project_name(),
            reduced: session.motion.reduced(),
        }
    }
}

impl Root {
    pub(crate) fn on_composer_request(
        &mut self,
        _: &Entity<Composer>,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Create(task) => {
                if self.session.snapshot.welcome {
                    self.session.create_first_task(task.clone());
                } else {
                    self.session.create(task.clone());
                }
            }
            ComposerEvent::Intake(notes, project) => {
                self.session.intake(notes.clone(), project.clone())
            }
            ComposerEvent::Toast(toast) => self.session.show_toast(toast.clone()),
            ComposerEvent::Menu(request) => {
                if let Some(menu) = request.borrow_mut().take() {
                    self.open_menu(
                        menu.entries,
                        menu.title,
                        menu.anchor,
                        menu.options,
                        window,
                        cx,
                    );
                }
            }
            ComposerEvent::Prompt(title, value, save) => self.open_prompt(
                title.clone(),
                "Value",
                "An empty value is different from no value.",
                value,
                save.clone(),
                window,
                cx,
            ),
        }
        cx.notify();
    }

    pub(crate) fn composer_open(&self, cx: &App) -> bool {
        self.composer.read(cx).composer_open()
    }

    pub(crate) fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        self.open_composer(window, cx);
    }

    pub(crate) fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = Source::from_session(&self.session);
        self.composer.update(cx, |composer, cx| {
            composer.set_source(source, cx);
            composer.open_composer(window, cx);
        });
    }

    pub(crate) fn render_composer(
        &self,
        _: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Composer>> {
        let source = Source::from_session(&self.session);
        self.composer
            .update(cx, |composer, cx| composer.set_source(source, cx));
        self.composer
            .read(cx)
            .visible()
            .then(|| self.composer.clone())
    }

    pub(crate) fn tint_composer_title(&self, cx: &mut App) {
        self.composer
            .update(cx, |composer, cx| composer.tint_composer_title(cx));
    }

    pub(crate) fn dismiss(
        &mut self,
        action: &Dismiss,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() {
            self.close_dialog(window, cx);
        } else {
            self.composer
                .update(cx, |composer, cx| composer.dismiss(action, window, cx));
        }
    }

    pub(crate) fn composer_fields(&self, cx: &App) -> Vec<gpui::FocusHandle> {
        self.composer.read(cx).composer_fields(cx)
    }

    pub(crate) fn restore_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = Source::from_session(&self.session);
        self.composer.update(cx, |composer, cx| {
            composer.set_source(source, cx);
            composer.restore_draft(window, cx);
        });
    }

    pub(crate) fn return_failed_draft(
        &mut self,
        draft: Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = Source::from_session(&self.session);
        self.composer.update(cx, |composer, cx| {
            composer.set_source(source, cx);
            composer.return_failed_draft(draft, window, cx);
        });
    }

    pub(crate) fn read_notes_back(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        let source = Source::from_session(&self.session);
        self.composer.update(cx, |composer, cx| {
            composer.set_source(source, cx);
            composer.read_notes_back(task, cx);
        });
    }
}

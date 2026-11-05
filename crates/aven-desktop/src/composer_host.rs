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

    /// Brings a launcher draft into Add Task, asking first when that would
    /// replace writing that was never saved.
    pub(crate) fn hand_off_draft(
        &mut self,
        workspace: String,
        task: Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.activate_window();
        cx.activate(true);
        self.close_menu(window, cx);
        let unsaved = self.composer.read(cx).unsaved_draft(cx);
        if !unsaved.has_text() && unsaved.images.is_empty() {
            self.take_launcher_draft(workspace, task, window, cx);
            return;
        }
        self.composer
            .update(cx, |composer, cx| composer.set_aside(window, cx));
        let mut body = "The launcher drafted a new task, but Add Task already has one you \
                        haven\u{2019}t saved"
            .to_string();
        match unsaved.title.trim() {
            "" => body.push('.'),
            title => body.push_str(&format!(": \u{201c}{title}\u{201d}")),
        }
        self.open_confirm(
            "Replace your unsaved draft?".to_string(),
            body,
            None,
            "Replace draft",
            crate::dialogs::ConfirmAction::ReplaceDraft {
                workspace,
                task: Box::new(task),
            },
            window,
            cx,
        );
    }

    /// Opens Add Task on a launcher draft, in the workspace it was written
    /// for, and lets the launcher forget it.
    pub(crate) fn take_launcher_draft(
        &mut self,
        workspace: String,
        task: Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_workspace_key() != Some(workspace.as_str()) {
            self.switch_view(
                crate::adapter::Command::SwitchWorkspace(workspace),
                window,
                cx,
            );
        }
        let source = Source::from_session(&self.session);
        self.composer.update(cx, |composer, cx| {
            composer.set_source(source, cx);
            composer.take_draft(Draft::from_task(task), window, cx);
        });
        cx.defer(crate::launcher::clear);
        cx.notify();
    }

    pub(crate) fn read_notes_back(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        let source = Source::from_session(&self.session);
        self.composer.update(cx, |composer, cx| {
            composer.set_source(source, cx);
            composer.read_notes_back(task, cx);
        });
    }
}

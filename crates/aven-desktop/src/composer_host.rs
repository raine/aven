//! The window's task-writing and popup boundary.
use gpui::{App, Context, Entity, Window};

use crate::actions::{Dismiss, NewTask};
use crate::adapter::NewTask as Task;
use crate::composer::{Composer, ComposerEvent, Source};
use crate::draft::{Draft, Kept};
use crate::launch::LaunchEvent;
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
            workspace: session
                .active_workspace_key()
                .unwrap_or_default()
                .to_string(),
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
            ComposerEvent::Launch(job) => self.session.read(job.clone()),
            ComposerEvent::CancelLaunch(job) => self.session.cancel_launch(*job),
            ComposerEvent::CreateUnreviewed { kept, task } => {
                self.session.create_unreviewed(kept.clone(), task.clone())
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
        let lead = "The launcher drafted a new task, but Add Task already has one you \
                    haven\u{2019}t saved";
        let action = crate::dialogs::ConfirmAction::ReplaceDraft {
            workspace: workspace.clone(),
            task: Box::new(task.clone()),
        };
        if !self.confirm_replace(lead, action, window, cx) {
            self.take_launcher_draft(workspace, task, window, cx);
        }
    }

    /// Asks before Add Task's unsaved writing is replaced, setting it aside
    /// meanwhile. False when there is nothing to lose, so nothing is asked.
    fn confirm_replace(
        &mut self,
        lead: &str,
        action: crate::dialogs::ConfirmAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let unsaved = self.composer.read(cx).unsaved_draft(cx);
        if unsaved.is_blank() {
            return false;
        }
        self.composer
            .update(cx, |composer, cx| composer.set_aside(window, cx));
        let mut body = lead.to_string();
        match unsaved.title.trim() {
            "" => body.push('.'),
            title => body.push_str(&format!(": \u{201c}{title}\u{201d}")),
        }
        self.open_confirm(
            "Replace your unsaved draft?".to_string(),
            body,
            None,
            "Replace draft",
            action,
            window,
            cx,
        );
        true
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
        self.open_draft_in(workspace, Draft::from_task(task), window, cx);
        cx.defer(crate::launcher::clear);
    }

    /// Opens Add Task on `draft` in `workspace`, in place of what it held.
    fn open_draft_in(
        &mut self,
        workspace: String,
        draft: Draft,
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
            composer.take_draft(draft, window, cx);
        });
        cx.notify();
    }

    /// Hands news of an agent job to Add Task when it started the job, and
    /// to the launcher otherwise.
    pub(crate) fn take_launch_event(&mut self, event: LaunchEvent, cx: &mut Context<Self>) {
        let source = Source::from_session(&self.session);
        let left = self.composer.update(cx, |composer, cx| {
            composer.set_source(source, cx);
            composer.take_launch_event(event, cx)
        });
        if let Some(event) = left {
            cx.defer(move |cx| crate::launcher::receive(event, cx));
        }
    }

    pub(crate) fn unreviewed_failed(
        &mut self,
        (kept, why): (Box<Kept>, String),
        cx: &mut Context<Self>,
    ) {
        self.composer.update(cx, |composer, cx| {
            composer.unreviewed_failed(*kept, why, cx);
        });
    }

    /// Opens the draft Add Task kept for agent job `job`, asking first when
    /// that would replace writing that was never saved.
    pub(crate) fn open_kept_draft(
        &mut self,
        job: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composer.read(cx).kept_for(job).is_none() {
            return;
        }
        let lead = "Add Task already has a draft you haven\u{2019}t saved";
        let action = crate::dialogs::ConfirmAction::OpenKeptDraft(job);
        if !self.confirm_replace(lead, action, window, cx) {
            self.take_kept_draft(job, window, cx);
        }
    }

    /// Opens Add Task on the draft kept for agent job `job`, in the
    /// workspace it was written in.
    pub(crate) fn take_kept_draft(
        &mut self,
        job: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(kept) = self
            .composer
            .update(cx, |composer, _| composer.take_kept(job))
        {
            self.open_draft_in(kept.workspace, kept.draft, window, cx);
        }
    }
}

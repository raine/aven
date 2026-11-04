//! Writing a new task: a title whose tokens turn into chips, a description,
//! and chips for every field. A draft survives closing until it is discarded.
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use accesskit::{Role, Toggled};
use aven_core::choices::TaskStatus;
use aven_core::projects::normalize_key;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, Entity, ExternalPaths, FocusHandle,
    KeyDownEvent, MouseButton, MouseDownEvent, Subscription, Window, div, ease_out_quint,
    prelude::*,
};

use crate::actions::*;
use crate::adapter::NavEntry;
use crate::commands::{pasted_image, read_image};
use crate::draft::{
    CaretWord, Draft, DraftProblem, Suggestion, Token, automatic_status, caret_word, cut,
    finished_token, parse_title, schedule_chip, schedule_preview, suggestions, title_tokens,
};
use crate::input::{Editor, EditorEvent, Mode, Tint};
use crate::kit::*;
use crate::overlays::{Entry, Mark, MenuOptions};
use crate::shell::Root;
use crate::theme::{Theme, theme};
use crate::toast::{Toast, ToastAction};

const DIALOG_CLOSE: Duration = Duration::from_millis(120);
const CHIP_FLASH: Duration = Duration::from_millis(600);
const COMPOSER_WIDTH: f32 = 600.;
const SUGGEST_WIDTH: f32 = 230.;
const SCHEDULE_WIDTH: f32 = 340.;
/// Roughly how tall the schedule popover stands, to open it upward when
/// there is no room below.
const SCHEDULE_HEIGHT: f32 = 210.;
const QUICK_SCHEDULES: [(&str, &str); 4] = [
    ("Tomorrow", "tomorrow"),
    ("Next Monday", "next monday"),
    ("Due Friday", "due friday"),
    ("Every Friday", "every friday at 09:00"),
];

/// A chip a title token can land in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chip {
    Project,
    Priority,
    Labels,
}

impl Chip {
    fn of(token: &Token) -> Self {
        match token {
            Token::Label(_) => Self::Labels,
            Token::Priority(_) => Self::Priority,
            Token::Project(_) => Self::Project,
        }
    }
}

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
    /// The notes out with the reader. A reply to anything else is stale.
    reading: Option<String>,
    /// The title as last seen, to tell a typed space from other edits.
    seen_title: String,
    /// The highlighted suggestion.
    suggestion: usize,
    /// The word whose suggestions Escape closed.
    dismissed: Option<CaretWord>,
    schedule_open: bool,
    /// The chip that just took a token, and that flash's number.
    flash: Option<(Chip, usize)>,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    /// The whole draft as it stands.
    fn current(&self, cx: &App) -> Draft {
        Draft {
            title: self.title.read(cx).text().to_string(),
            schedule: self.schedule.read(cx).text().to_string(),
            body: self.body.read(cx).text().to_string(),
            ..self.draft.clone()
        }
    }

    fn show(&mut self, draft: Draft, projects: &[NavEntry], cx: &mut Context<Root>) {
        self.title
            .update(cx, |editor, cx| editor.set_text(&draft.title, cx));
        self.schedule
            .update(cx, |editor, cx| editor.set_text(&draft.schedule, cx));
        self.body
            .update(cx, |editor, cx| editor.set_text(&draft.body, cx));
        self.draft = draft;
        self.seen_title = self.title.read(cx).text().to_string();
        self.suggestion = 0;
        self.dismissed = None;
        self.schedule_open = false;
        self.tint_title(projects, cx);
    }

    /// Colours the title's tokens as what they set.
    fn tint_title(&self, projects: &[NavEntry], cx: &mut App) {
        let t = theme(cx);
        let tints = title_tokens(self.title.read(cx).text(), projects)
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
        self.title
            .update(cx, |editor, cx| editor.set_tints(tints, cx));
    }

    /// The labels the draft holds, chosen or in the title, leaving out the
    /// token word starting at `typing`.
    fn labels(&self, projects: &[NavEntry], typing: Option<usize>, cx: &App) -> Vec<String> {
        let mut labels = self.draft.labels.clone();
        for (range, token) in title_tokens(self.title.read(cx).text(), projects) {
            if let Token::Label(label) = token
                && Some(range.start) != typing
                && !labels.contains(&label)
            {
                labels.push(label);
            }
        }
        labels
    }
}

impl Root {
    pub(crate) fn composer_open(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| !composer.closing)
    }

    pub(crate) fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        self.open_composer(window, cx);
    }

    pub(crate) fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer_open() {
            return;
        }
        let draft = std::mem::take(&mut self.kept_draft);
        let title = cx.new(|cx| {
            let placeholder = "What needs doing? #label !high @project";
            Editor::new(Mode::Single, placeholder, window, cx).takes_images()
        });
        let schedule = cx.new(|cx| {
            let placeholder = "tomorrow, due next friday, every friday at 09:00";
            Editor::new(Mode::Single, placeholder, window, cx)
        });
        let body = cx.new(|cx| {
            let placeholder = "Add a description. Markdown works; paste or drop images.";
            Editor::new(Mode::Multi, placeholder, window, cx)
                .enter_breaks()
                .takes_images()
                .max_height(px(280.))
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
            reading: None,
            seen_title: String::new(),
            suggestion: 0,
            dismissed: None,
            schedule_open: false,
            flash: None,
            _subscriptions: subscriptions,
        };
        composer.show(draft, &self.session.snapshot.projects, cx);
        self.composer = Some(composer);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Recolours the title's tokens, for a theme they were not drawn in.
    pub(crate) fn tint_composer_title(&self, cx: &mut App) {
        if let Some(composer) = self.composer.as_ref() {
            composer.tint_title(&self.session.snapshot.projects, cx);
        }
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

    pub(crate) fn composer_draft(&self, cx: &App) -> Option<Draft> {
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
            self.session
                .show_toast(Toast::done("Draft discarded").action(Some(ToastAction::RestoreDraft)));
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

    /// Whether creating leaves the composer open: never for a repeating task,
    /// which is created on its own.
    fn creates_more(&self, cx: &App) -> bool {
        self.create_more && !self.composer_draft(cx).is_some_and(|draft| draft.repeats())
    }

    fn submit_composer(&mut self, another: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.composer_draft(cx) else {
            return;
        };
        match draft.task(&self.session.snapshot.projects) {
            Ok(mut task) => {
                // Left to inference, the task joins the project the chip shows.
                task.project = task
                    .project
                    .or_else(|| Some(self.session.new_task_project()?.0.to_string()));
                if self.session.snapshot.welcome {
                    self.session.create_first_task(task);
                } else {
                    self.session.create(task);
                }
                if another {
                    if let Some(composer) = self.composer.as_mut() {
                        let next = draft.after_created();
                        let focus = composer.title.read(cx).focus.clone();
                        composer.show(next, &self.session.snapshot.projects, cx);
                        window.focus(&focus, cx);
                    }
                    cx.notify();
                } else {
                    self.close_composer(None, window, cx);
                }
            }
            Err(problem @ DraftProblem::Title) => {
                let title = self
                    .composer
                    .as_ref()
                    .map(|composer| composer.title.read(cx).focus.clone());
                self.refuse_draft(problem, title, window, cx);
            }
            Err(problem @ DraftProblem::RecurringAttachments) => {
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
            Err(DraftProblem::Schedule) => self.open_schedule(window, cx),
        }
    }

    fn refuse_draft(
        &mut self,
        problem: DraftProblem,
        focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session.show_toast(problem.toast());
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
        let more = self.creates_more(cx);
        self.submit_composer(more, window, cx);
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

    fn toggle_create_more(&mut self, cx: &mut Context<Self>) {
        self.create_more = !self.create_more;
        self.mark_switched("c-create-more");
        cx.notify();
    }

    pub(crate) fn read_notes(&mut self, _: &ReadNotes, _: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.composer_draft(cx) else {
            return;
        };
        let notes = draft.intake_text();
        if notes.is_empty() {
            self.session
                .show_toast(Toast::info("Add rough notes first"));
        } else {
            self.session.intake(notes.clone(), draft.project.clone());
            if let Some(composer) = self.composer.as_mut() {
                composer.reading = Some(notes);
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
        let Some(notes) = composer.reading.take() else {
            return;
        };
        if let Some(task) = task {
            let mut draft = composer.current(cx);
            if draft.fill_from_notes(&notes, task) {
                composer.show(draft, &self.session.snapshot.projects, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn composer_fields(&self, cx: &Context<Self>) -> Vec<FocusHandle> {
        let Some(composer) = self.composer.as_ref() else {
            return Vec::new();
        };
        if composer.schedule_open {
            vec![composer.schedule.read(cx).focus.clone()]
        } else {
            vec![
                composer.title.read(cx).focus.clone(),
                composer.body.read(cx).focus.clone(),
            ]
        }
    }

    fn on_composer_event(
        &mut self,
        editor: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(composer) = self.composer.as_ref() else {
            return;
        };
        let is_title = *editor == composer.title;
        let is_schedule = *editor == composer.schedule;
        match event {
            // Enter in the description breaks the line, so only one-line fields
            // reach here.
            EditorEvent::Submit | EditorEvent::Cancel if is_schedule => {
                self.close_schedule(true, window, cx);
            }
            EditorEvent::Blur if is_schedule => self.close_schedule(false, window, cx),
            EditorEvent::Submit => self.create_task(&CreateTask, window, cx),
            EditorEvent::Cancel => self.dismiss(&Dismiss, window, cx),
            EditorEvent::Changed if is_title => self.title_changed(window, cx),
            EditorEvent::Changed => cx.notify(),
            EditorEvent::PastedImage(pasted) => {
                if let (Some(image), Some(composer)) =
                    (pasted_image(pasted.clone()), self.composer.as_mut())
                {
                    composer.draft.add_image(image);
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Moves a token just finished with a space out of the title.
    fn title_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let projects = &self.session.snapshot.projects;
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        let editor = composer.title.read(cx);
        let now = editor.text().to_string();
        let finished = (editor.typed() && !editor.is_composing())
            .then(|| finished_token(&composer.seen_title, &now, editor.cursor(), projects))
            .flatten();
        composer.seen_title = now;
        composer.suggestion = 0;
        composer.tint_title(projects, cx);
        if let Some((range, token)) = finished {
            self.take_token(range, token, window, cx);
        }
        cx.notify();
    }

    /// Cuts `range` out of the title and sets the field `token` names.
    fn take_token(
        &mut self,
        range: Range<usize>,
        token: Token,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        let (range, glue) = cut(composer.title.read(cx).text(), range);
        composer
            .title
            .update(cx, |editor, cx| editor.replace(range, glue, window, cx));
        let chip = Chip::of(&token);
        composer.draft.apply(token);
        composer.dismissed = None;
        self.flash_chip(chip, cx);
    }

    /// Marks the chip a token landed in for a moment.
    fn flash_chip(&mut self, chip: Chip, cx: &mut Context<Self>) {
        if self.session.motion.reduced() {
            return;
        }
        let run = self.session.motion.next_run();
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        composer.flash = Some((chip, run));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CHIP_FLASH).await;
            this.update(cx, |this, cx| {
                let composer = this.composer.as_mut();
                if let Some(composer) = composer.filter(|c| c.flash.is_some_and(|f| f.1 == run)) {
                    composer.flash = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// What the word at the title's caret could become, while it is focused
    /// and being typed.
    fn composer_suggestions(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(CaretWord, Vec<Suggestion>)> {
        let composer = self
            .composer
            .as_ref()
            .filter(|composer| !composer.closing && !composer.schedule_open)?;
        let editor = composer.title.read(cx);
        if !editor.focus.is_focused(window) || editor.has_selection() || editor.is_composing() {
            return None;
        }
        let word = caret_word(editor.text(), editor.cursor())?;
        if composer.dismissed.as_ref() == Some(&word) {
            return None;
        }
        let projects = &self.session.snapshot.projects;
        let held = composer.labels(projects, Some(word.start), cx);
        let found = suggestions(&word, &held, &self.session.snapshot.labels, projects);
        (!found.is_empty()).then_some((word, found))
    }

    fn move_suggestion(&mut self, step: isize, window: &Window, cx: &mut Context<Self>) {
        let Some((_, found)) = self.composer_suggestions(window, cx) else {
            return;
        };
        if let Some(composer) = self.composer.as_mut() {
            let count = found.len() as isize;
            let at = composer.suggestion.min(found.len() - 1) as isize;
            composer.suggestion = (at + step).rem_euclid(count) as usize;
        }
        cx.notify();
    }

    pub(crate) fn suggest_next(
        &mut self,
        _: &SuggestNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_suggestion(1, window, cx);
    }

    pub(crate) fn suggest_previous(
        &mut self,
        _: &SuggestPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_suggestion(-1, window, cx);
    }

    pub(crate) fn accept_suggestion(
        &mut self,
        _: &AcceptSuggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = self
            .composer
            .as_ref()
            .map_or(0, |composer| composer.suggestion);
        self.take_suggestion(at, window, cx);
    }

    /// Takes suggestion `at` in place of the whole word under the caret.
    fn take_suggestion(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((word, mut found)) = self.composer_suggestions(window, cx) else {
            return;
        };
        let Some(composer) = self.composer.as_ref() else {
            return;
        };
        let editor = composer.title.read(cx);
        let rest = &editor.text()[editor.cursor()..];
        let end = editor.cursor() + rest.find(char::is_whitespace).unwrap_or(rest.len());
        let suggestion = found.swap_remove(at.min(found.len() - 1));
        self.take_token(word.start..end, suggestion.token, window, cx);
        cx.notify();
    }

    pub(crate) fn close_suggestions(
        &mut self,
        _: &CloseSuggestions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let word = self.composer_suggestions(window, cx).map(|(word, _)| word);
        if let Some(composer) = self.composer.as_mut() {
            composer.dismissed = word;
        }
        cx.notify();
    }

    /// Moves the title's tokens into the fields, so a chip shows and edits
    /// what creating would use.
    fn settle_composer_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let projects = &self.session.snapshot.projects;
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        let mut draft = composer.current(cx);
        let before = draft.title.len();
        draft.settle_title(projects);
        if draft.title.len() != before {
            let title = draft.title.clone();
            composer.title.update(cx, |editor, cx| {
                editor.replace(0..before, &title, window, cx)
            });
            composer.draft = draft;
        }
    }

    fn open_schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(composer) = self.composer.as_mut() {
            composer.schedule_open = true;
            let focus = composer.schedule.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// Closes the schedule popover, back to the title when `refocus` or when
    /// focus would otherwise go with the popover.
    fn close_schedule(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composer) = self.composer.as_mut().filter(|c| c.schedule_open) else {
            return;
        };
        composer.schedule_open = false;
        if refocus || composer.schedule.read(cx).focus.is_focused(window) {
            let focus = composer.title.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Draft)) {
        if let Some(composer) = self.composer.as_mut() {
            change(&mut composer.draft);
        }
        cx.notify();
    }

    fn pick_composer_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settle_composer_title(window, cx);
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
                .mark(Mark::Project(project.key.clone())),
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
        self.settle_composer_title(window, cx);
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
        self.settle_composer_title(window, cx);
        let held = self
            .composer_draft(cx)
            .map(|draft| draft.labels)
            .unwrap_or_default();
        type Save = Rc<dyn Fn(&mut Root, &mut Context<Root>)>;
        let save: Save = Rc::new(|root, cx| {
            let labels = root.menu_checked();
            root.update_draft(cx, |draft| draft.labels = labels);
        });
        let entry =
            move |label: String, save: Save| Entry::new(label, move |root, _, cx| save(root, cx));
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
        let anchor = self.anchor("c-more");
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

    /// The ⋯ menu: what no chip shows until it is set.
    fn pick_composer_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_epic = self.composer_draft(cx).is_some_and(|draft| draft.is_epic);
        let entries = vec![
            Entry::new(
                if is_epic {
                    "Make it a task"
                } else {
                    "Make it an epic"
                },
                |root, _, cx| root.toggle_composer_epic(cx),
            ),
            Entry::new("Add metadata…", |root, window, cx| {
                root.pick_composer_metadata(window, cx);
            }),
            Entry::new("Attach image…", |root, _, cx| {
                root.prompt_for_images(cx, Self::drop_on_composer);
            }),
        ];
        let anchor = self.anchor("c-more");
        let options = MenuOptions {
            plain: true,
            ..MenuOptions::default()
        };
        self.open_menu(entries, "More", anchor, options, window, cx);
    }

    fn drop_on_composer(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let images: Vec<_> = paths.iter().filter_map(|path| read_image(path)).collect();
        self.update_draft(cx, |draft| {
            for image in images {
                draft.add_image(image);
            }
        });
    }

    fn chip(
        id: &'static str,
        label: impl Into<gpui::SharedString>,
        set: bool,
        anchors: &Anchors,
        t: &Theme,
    ) -> gpui::Stateful<gpui::Div> {
        btn(id, label, t)
            .relative()
            .h(px(26.))
            .px(px(8.))
            .text_size(px(12.))
            .when(set, |chip| chip.shadow(vec![t.inset_ring(t.line2)]))
            .when(!set, |chip| {
                chip.text_color(t.faint).shadow(vec![t.inset_ring(t.line)])
            })
            .child(anchor_probe(id, anchors))
    }

    /// The fading mark over a chip that just took a token.
    fn chip_flash(chip: Chip, flash: Option<(Chip, usize)>, t: &Theme) -> Option<AnyElement> {
        let (_, run) = flash.filter(|(flashed, _)| *flashed == chip)?;
        Some(
            div()
                .absolute()
                .inset_0()
                .rounded(px(6.))
                .bg(t.accent.opacity(0.22))
                .shadow(vec![t.inset_ring(t.tint_line)])
                .with_animation(
                    ("chip-flash", run),
                    Animation::new(CHIP_FLASH).with_easing(ease_out_quint()),
                    |mark, delta| mark.opacity(1. - delta),
                )
                .into_any_element(),
        )
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
        let projects = &self.session.snapshot.projects;
        let parts = parse_title(&draft.title, projects);
        let priority = parts.priority.unwrap_or(draft.priority);
        let flash = composer.flash;
        let find = |key: &str| projects.iter().find(|project| project.key == key);
        let chosen = parts.project.as_deref().or(draft.project.as_deref());
        let project = chosen
            .and_then(find)
            .map(|project| (project, None))
            .or_else(|| {
                let (key, why) = self.session.new_task_project()?;
                Some((find(key)?, Some(why)))
            });
        // A workspace with no projects gets one with the task.
        let first =
            (project.is_none() && projects.is_empty()).then(|| self.session.first_project_name());
        let labels = composer.labels(projects, None, cx);
        let anchors = &self.anchors;
        let schedule = schedule_preview(&draft.schedule);
        let status = draft
            .status
            .unwrap_or_else(|| automatic_status(priority, schedule.repeats, draft.is_epic));
        let schedule_text = schedule_chip(&draft.schedule);
        let has_schedule = !draft.schedule.trim().is_empty();

        let project_chip = match project {
            Some((project, inferred)) => {
                Self::chip("c-project", project.name.clone(), true, anchors, &t)
                    .when_some(inferred, |chip, why| chip.tooltip(tip(why)))
                    .child(project_dot(&project.key, &t))
                    .child(project.name.clone())
            }
            None => match first {
                Some(name) => {
                    let label = format!("{name} (new)");
                    Self::chip("c-project", label.clone(), true, anchors, &t)
                        .tooltip(tip("Created with this task"))
                        .child(project_dot(&normalize_key(&name), &t))
                        .child(label)
                }
                None => Self::chip("c-project", "Project", false, anchors, &t).child("Project"),
            },
        }
        .children(Self::chip_flash(Chip::Project, flash, &t))
        .on_click(cx.listener(|this, _, window, cx| this.pick_composer_project(window, cx)));

        let status_label = if draft.status.is_some() {
            status_name(status).to_string()
        } else {
            format!("{} (automatic)", status_name(status))
        };
        let status_chip = Self::chip("c-status", status_label, true, anchors, &t)
            .child(status_glyph(status, &t))
            .child(
                div()
                    .when(draft.status.is_none(), |name| name.text_color(t.muted))
                    .child(status_name(status)),
            )
            .on_click(cx.listener(|this, _, window, cx| this.pick_composer_status(window, cx)))
            .track_focus(&composer.status)
            .when(composer.status.is_focused(window), |chip| {
                chip.shadow(vec![t.inset_ring(t.accent)])
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.pick_composer_status(window, cx);
                }
            }));

        let priority_set = priority != aven_core::choices::TaskPriority::None;
        let priority_label = if priority_set {
            priority_name(priority)
        } else {
            "Priority"
        };
        let priority_chip = Self::chip("c-priority", priority_label, priority_set, anchors, &t)
            .child(priority_icon(priority, &t))
            .child(priority_label)
            .children(Self::chip_flash(Chip::Priority, flash, &t))
            .on_click(cx.listener(|this, _, window, cx| this.pick_composer_priority(window, cx)));

        let labels_text = if labels.is_empty() {
            "Labels".to_string()
        } else {
            labels.join(", ")
        };
        let labels_chip = Self::chip(
            "c-labels",
            labels_text.clone(),
            !labels.is_empty(),
            anchors,
            &t,
        )
        .child(
            icon(
                "icons/ic-labels.svg",
                if labels.is_empty() { t.faint } else { t.muted },
            )
            .size(px(14.)),
        )
        .child(labels_text)
        .children(Self::chip_flash(Chip::Labels, flash, &t))
        .on_click(cx.listener(|this, _, window, cx| this.pick_composer_labels(window, cx)));

        let schedule_label = schedule_text.clone().unwrap_or_else(|| {
            if has_schedule {
                draft.schedule.trim().to_string()
            } else {
                "Schedule".to_string()
            }
        });
        let schedule_icon = if schedule.repeats {
            "icons/ic-recurring.svg"
        } else {
            "icons/ic-upcoming.svg"
        };
        let schedule_chip = Self::chip(
            "c-schedule",
            schedule_label.clone(),
            has_schedule,
            anchors,
            &t,
        )
        .when(!schedule.valid, |chip| {
            chip.text_color(t.bad)
                .shadow(vec![t.inset_ring(t.bad.opacity(0.5))])
        })
        .child(icon(schedule_icon, if has_schedule { t.muted } else { t.faint }).size(px(14.)))
        .child(schedule_label)
        .on_click(cx.listener(|this, _, window, cx| {
            if this.composer.as_ref().is_some_and(|c| c.schedule_open) {
                this.close_schedule(true, window, cx);
            } else {
                this.open_schedule(window, cx);
            }
        }));

        let mut chips = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(5.))
            .px(px(18.))
            .pt(px(2.))
            .pb(px(16.))
            .child(project_chip)
            .child(status_chip)
            .child(priority_chip)
            .child(labels_chip)
            .child(schedule_chip)
            .when(draft.is_epic, |row| {
                row.child(
                    Self::chip("c-epic", "Epic, make it a task", true, anchors, &t)
                        .text_color(t.accent)
                        .shadow(vec![t.inset_ring(t.tint_line)])
                        .child(icon("icons/ic-epics.svg", t.accent).size(px(14.)))
                        .child("Epic")
                        .child(div().text_color(t.faint).child("×"))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_composer_epic(cx)))
                        .track_focus(&composer.epic)
                        .when(composer.epic.is_focused(window), |chip| {
                            chip.shadow(vec![t.inset_ring(t.accent)])
                        })
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.toggle_composer_epic(cx);
                            }
                        })),
                )
            });
        for (ix, (key, value)) in draft.metadata.iter().enumerate() {
            let shown = if value.is_empty() {
                "\"\""
            } else {
                value.as_str()
            };
            let text = format!("{key}: {shown}");
            let key = key.clone();
            chips = chips.child(
                btn(("c-meta", ix), text.clone(), &t)
                    .h(px(26.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .shadow(vec![t.inset_ring(t.line2)])
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.prompt_composer_metadata(key.clone(), window, cx);
                    }))
                    .child(text),
            );
        }
        let chips = chips.child(
            Self::chip("c-more", "More options", false, anchors, &t)
                .w(px(26.))
                .px(px(0.))
                .justify_center()
                .child("⋯")
                .on_click(cx.listener(|this, _, window, cx| this.pick_composer_more(window, cx))),
        );

        let mut images = div().flex().flex_wrap().gap(px(6.)).px(px(18.)).pb(px(10.));
        for (ix, image) in draft.images.iter().enumerate() {
            images = images.child(
                btn(("c-image", ix), format!("Remove {}", image.name), &t)
                    .h(px(28.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .shadow(vec![t.inset_ring(t.line2)])
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_draft(cx, |draft| {
                            draft.images.remove(ix);
                        });
                    }))
                    .child(image.name.clone())
                    .child(div().text_color(t.faint).child("×")),
            );
        }

        let repeats = schedule.repeats;
        let create_more = self.create_more && !repeats;
        let switch = div()
            .id("c-create-more-row")
            .role(Role::Switch)
            .aria_label("Create more")
            .aria_toggled(if create_more {
                Toggled::True
            } else {
                Toggled::False
            })
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(8.))
            .rounded(px(6.))
            .text_color(t.muted)
            .when(repeats, |row| {
                row.opacity(0.45)
                    .tooltip(tip("Repeating tasks are created one at a time"))
            })
            .when(!repeats, |row| {
                let raised = t.raised;
                let ink = t.ink;
                row.hover(move |style| style.bg(raised).text_color(ink))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_create_more(cx)))
            })
            .child(self.switch_track(
                div().id("c-create-more"),
                "c-create-more",
                create_more,
                0.7,
                cx,
                &t,
            ))
            .child("Create more");
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
                    if composer.reading.is_some() {
                        "Reading…"
                    } else {
                        "Create with AI"
                    },
                    &t,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.read_notes(&ReadNotes, window, cx);
                }))
                .child(if composer.reading.is_some() {
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
            .child(switch)
            .child(
                fill_btn("c-create", "Create", &t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.create_task(&CreateTask, window, cx);
                    }))
                    .child(kbd("⌘↵", t.on_fill)),
            );

        let suggesting = self.composer_suggestions(window, cx);
        let title = div()
            .relative()
            .flex_none()
            .h(px(30.))
            .mx(px(18.))
            .mt(px(12.))
            .flex()
            .items_center()
            .text_size(px(16.))
            .text_color(t.ink)
            .when(suggesting.is_some(), |row| row.key_context("Suggesting"))
            .child(anchor_probe("c-title", anchors))
            .child(composer.title.clone());
        let body = div()
            .flex_none()
            .min_h(px(44.))
            .px(px(18.))
            .pt(px(6.))
            .pb(px(10.))
            .text_color(t.body)
            .child(composer.body.clone());

        let width = COMPOSER_WIDTH.min(f32::from(viewport.width) - 48.);
        let panel = div()
            .id("composer")
            .role(Role::Dialog)
            .aria_label("New task")
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
                panel.occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.close_schedule(false, window, cx);
                    }),
                )
            })
            .child(title)
            .child(body)
            .when(!draft.images.is_empty(), |panel| panel.child(images))
            .child(chips)
            .child(actions);

        let suggest = suggesting.map(|(word, found)| {
            self.render_suggestions(composer, &word, &found, viewport, cx, &t)
        });
        let schedule_pop = composer
            .schedule_open
            .then(|| self.render_schedule(composer, &schedule, viewport, cx, &t));
        let layer = div()
            .id("composer-backdrop")
            .key_context("Dialog Composer")
            .on_action(cx.listener(Self::create_task))
            .on_action(cx.listener(Self::create_another))
            .on_action(cx.listener(Self::read_notes))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::next_field))
            .on_action(cx.listener(Self::previous_field))
            .on_action(cx.listener(Self::suggest_next))
            .on_action(cx.listener(Self::suggest_previous))
            .on_action(cx.listener(Self::accept_suggestion))
            .on_action(cx.listener(Self::close_suggestions))
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
            .child(panel)
            .children(suggest)
            .children(schedule_pop);
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

    /// The list of ways to finish the title's token word, under its start.
    fn render_suggestions(
        &self,
        composer: &Composer,
        word: &CaretWord,
        found: &[Suggestion],
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> AnyElement {
        let picked = composer.suggestion.min(found.len() - 1);
        let row = self.anchor("c-title");
        let x = composer
            .title
            .read(cx)
            .offset_origin(word.start)
            .map_or(row.left(), |origin| origin.x);
        let left = (x - px(8.))
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
                    .id(("c-suggestion", ix))
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
                            small(12.)
                                .ml_auto()
                                .flex_none()
                                .text_color(t.faint)
                                .child(suggestion.hint.clone()),
                        )
                    }),
            );
        }
        div()
            .id("c-suggestions")
            .role(Role::List)
            .absolute()
            .left(left)
            .top(row.bottom() + px(2.))
            .w(px(SUGGEST_WIDTH))
            .rounded(px(10.))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .text_size(px(13.))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(rows)
            .into_any_element()
    }

    /// The schedule popover under its chip: typed text, quick picks and what
    /// the text means.
    fn render_schedule(
        &self,
        composer: &Composer,
        schedule: &crate::draft::SchedulePreview,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> AnyElement {
        let chip = self.anchor("c-schedule");
        let left = chip
            .left()
            .min(viewport.width - px(SCHEDULE_WIDTH + 12.))
            .max(px(12.));
        let below = chip.bottom() + px(4.);
        let top = if below + px(SCHEDULE_HEIGHT) > viewport.height - px(12.) {
            chip.top() - px(SCHEDULE_HEIGHT + 4.)
        } else {
            below
        };
        div()
            .id("c-schedule-pop")
            .role(Role::Dialog)
            .aria_label("Schedule")
            .absolute()
            .left(left)
            .top(top)
            .w(px(SCHEDULE_WIDTH))
            .p(px(12.))
            .rounded(px(10.))
            .bg(t.pop)
            .shadow(t.menu_shadow())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(small(12.).mb(px(8.)).text_color(t.faint).child("Schedule"))
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
                    t,
                ),
            )
            .child(
                div()
                    .mt(px(8.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(7.))
                    .bg(t.raised)
                    .text_size(px(12.))
                    .text_color(if schedule.valid { t.body } else { t.bad })
                    .child(schedule.text.clone()),
            )
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        btn("c-schedule-clear", "Clear", t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(composer) = this.composer.as_ref() {
                                    let schedule = composer.schedule.clone();
                                    schedule.update(cx, |editor, cx| editor.set_text("", cx));
                                }
                                this.close_schedule(true, window, cx);
                            }))
                            .child("Clear"),
                    )
                    .child(
                        fill_btn("c-schedule-done", "Done", t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_schedule(true, window, cx);
                            }))
                            .child(kbd("↵", t.on_fill)),
                    ),
            )
            .into_any_element()
    }
}

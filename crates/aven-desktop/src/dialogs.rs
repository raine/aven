//! Dialogs over the window: search, prompts, images, help and the incoming
//! description. One shows at a time, above the composer, and focus goes
//! back where it came from.
use std::path::PathBuf;
use std::rc::Rc;

use accesskit::Role;
use aven_core::ids::TaskId;
use aven_core::recurrence::RecurrenceSeriesId;
use aven_host::store::ConflictTarget;
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, FocusHandle, FontWeight, HighlightStyle,
    MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, ScrollHandle, SharedString, StyledText,
    Subscription, UnderlineStyle, Window, div, img, prelude::*,
};

use crate::actions::*;
use crate::adapter::{Command, Image, SearchHit, SearchInput, SyncPhase};
use crate::input::{Editor, EditorEvent, Mode};
use crate::kit::*;
use crate::overlays::reveal_item;
use crate::session::SyncPage;
use crate::shell::Root;
use crate::theme::{Theme, theme};
use crate::toast::Toast;

const DIALOG_WIDTH: f32 = 560.;
const IMAGE_DIALOG_WIDTH: f32 = 860.;
/// What an image dialog needs besides the picture.
const IMAGE_DIALOG_CHROME: f32 = 190.;
const WIDE_WIDTH: f32 = 760.;
const NARROW_WIDTH: f32 = 440.;
const ABOUT_WIDTH: f32 = 300.;
const ABOUT_LINKS: [(&str, &str); 3] = [
    ("GitHub", "https://github.com/raine/aven"),
    (
        "Release notes",
        "https://github.com/raine/aven/blob/main/CHANGELOG.md",
    ),
    ("License", "https://github.com/raine/aven/blob/main/LICENSE"),
];
const SEARCH_WIDTH: f32 = 660.;
const SEARCH_LIST_HEIGHT: f32 = 312.;
const SEARCH_ROW_HEIGHT: f32 = 50.;
const SEARCH_LIST_PADDING: f32 = 6.;

pub(crate) type SavePromptText = Rc<dyn Fn(&mut Root, String, &mut Window, &mut Context<Root>)>;

const HELP: [(&str, &[(&str, &str)]); 6] = [
    (
        "Move",
        &[
            ("Next, previous", "j / k"),
            ("Expand task", "↩"),
            ("Previous, next", "[ / ]"),
            ("Close", "Esc"),
            ("All projects", "g a"),
            ("Project", "g p"),
            ("Workspace", "g w"),
            ("Toggle inspector", "g d"),
            ("Toggle sidebar", "g b"),
            ("Jump to blocker", "g B"),
            ("Jump to epic", "g e"),
            ("Focus epic tasks", "c"),
        ],
    ),
    (
        "Change",
        &[
            ("New task", "a"),
            ("Status", "s, then i b t a d x"),
            ("Priority", "!, then n l m h u"),
            ("Done", "d"),
            ("Canceled", "x"),
            ("Mark", "Space"),
            ("Extend marks", "⇧J / ⇧K"),
            ("Mark visible", "t V"),
            ("Clear marks", "t C"),
            ("Add to epic", "t c"),
            ("Note", "n"),
            ("Undo", "u"),
            ("Toast action", "⌘↩"),
            ("Title", "e t"),
            ("Description", "e d"),
            ("Project", "e j"),
            ("Availability", "e a"),
            ("Due date", "e u"),
            ("Labels", "e l"),
            ("Metadata", "e m"),
            ("Priority", "e p"),
        ],
    ),
    (
        "Task",
        &[
            ("Delete", "t D"),
            ("Restore", "t R"),
            ("Blockers", "t B"),
            ("Remove blocker", "t U"),
            ("Related", "t l"),
            ("Move lane", "< / >"),
        ],
    ),
    (
        "Views",
        &[
            ("Queue", "v q"),
            ("Ready", "v y"),
            ("Inbox", "v i"),
            ("Upcoming", "v p"),
            ("Overdue", "v x"),
            ("Blocked", "v k"),
            ("Epics", "v e"),
            ("Recurring", "v u"),
            ("Done", "v d"),
            ("Conflicts", "v c"),
            ("Recent", "v r"),
            ("Columns or list", "v l"),
        ],
    ),
    (
        "Find",
        &[
            ("Search", "/"),
            ("Palette", "⌘K or :"),
            ("Filter label", "f l"),
            ("Filter priority", "f p"),
            ("Deleted", "f x"),
            ("Metadata", "f m"),
            ("Recurring state", "f r"),
            ("Clear", "f c"),
            ("Order", "o, then p d a c u j t, r reverses"),
        ],
    ),
    (
        "Copy and sync",
        &[
            ("Ref", "y r"),
            ("ID", "y i"),
            ("Title", "y t"),
            ("Description", "y d"),
            ("Notes", "y n"),
            ("Report", "y m"),
            ("Secret gist", "t g"),
            ("This help", "?"),
        ],
    ),
];

pub enum Dialog {
    Search {
        input: Entity<Editor>,
        all: bool,
        deleted: bool,
        index: usize,
        scroll: ScrollHandle,
    },
    /// What becomes of a stopped series' current task.
    Stop {
        id: RecurrenceSeriesId,
        title: String,
        current: String,
        /// Cancel, keep and skip, in Tab order.
        buttons: [FocusHandle; 3],
    },
    /// One task's conflicts, walked field by field.
    Resolve {
        targets: Vec<ConflictTarget>,
        at: usize,
        input: Entity<Editor>,
        chosen: Option<usize>,
        sides: [FocusHandle; 2],
        /// Decide later, keep and next, and keep this value, in Tab order.
        buttons: [FocusHandle; 3],
    },
    Gist {
        id: TaskId,
        /// Cancel and publish, in Tab order.
        buttons: [FocusHandle; 2],
    },
    Prompt {
        title: String,
        label: &'static str,
        hint: &'static str,
        input: Entity<Editor>,
        save: SavePromptText,
    },
    Image {
        id: TaskId,
        image: Image,
        alt: Entity<Editor>,
    },
    Help,
    About,
    /// Pages come from `Session::sync_page`; these outlive a page change.
    Sync {
        input: Entity<Editor>,
        buttons: [FocusHandle; 2],
        /// The page the Tab order was last built for.
        page: Option<std::mem::Discriminant<SyncPage>>,
    },
    Confirm {
        title: String,
        body: String,
        note: Option<&'static str>,
        label: &'static str,
        cancel: &'static str,
        action: ConfirmAction,
        /// Cancel and confirm, in Tab order.
        buttons: [FocusHandle; 2],
    },
    Project {
        key: Option<String>,
        name: Entity<Editor>,
        prefix: Entity<Editor>,
        error: Option<&'static str>,
    },
    Incoming {
        id: TaskId,
        mine: String,
        theirs: String,
    },
    /// The app's own update, in whatever phase it is.
    Update {
        /// The secondary and primary buttons, in Tab order.
        buttons: [FocusHandle; 2],
    },
}

/// What a yes in a confirmation dialog does.
#[derive(Clone)]
pub enum ConfirmAction {
    DeleteProject(String),
    RemoveDevice {
        id: [u8; 32],
        label: String,
    },
    Restore(PathBuf),
    Reset {
        unfinished_setup: bool,
    },
    /// Puts a launcher draft for the workspace with this key in Add Task.
    ReplaceDraft {
        workspace: String,
        task: Box<crate::adapter::NewTask>,
    },
    /// Puts the draft Add Task kept for this agent job in Add Task.
    OpenKeptDraft(u64),
    /// Quits after saying what keeps running. Cancel opens Settings instead.
    Quit,
}

pub struct Layer {
    pub dialog: Dialog,
    prior: Option<FocusHandle>,
    /// The dialog's own handle, since a focusable element reports its handle
    /// and each field already reports the one in `fields`.
    container: FocusHandle,
    pub(crate) fields: Vec<FocusHandle>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Root {
    pub(crate) fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub(crate) fn open_dialog(
        &mut self,
        dialog: Dialog,
        editors: Vec<Entity<Editor>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fields = editors
            .iter()
            .map(|editor| editor.read(cx).focus.clone())
            .collect();
        self.open_dialog_at(dialog, editors, fields, 0, window, cx);
    }

    /// Opens a dialog whose Tab order is `fields`, starting on `first`.
    pub(crate) fn open_dialog_at(
        &mut self,
        dialog: Dialog,
        editors: Vec<Entity<Editor>>,
        fields: Vec<FocusHandle>,
        first: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscriptions = editors
            .iter()
            .map(|editor| cx.subscribe_in(editor, window, Self::on_dialog_event))
            .collect();
        let container = cx.focus_handle();
        let prior = window.focused(cx);
        window.focus(fields.get(first).unwrap_or(&container), cx);
        self.dialog = Some(Layer {
            dialog,
            prior,
            container,
            fields,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.dialog.take() {
            if matches!(layer.dialog, Dialog::Sync { .. }) {
                self.session.sync_closed();
            }
            let focus = layer.prior.unwrap_or_else(|| self.focus.clone());
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn on_dialog_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let searching = matches!(
            self.dialog.as_ref().map(|layer| &layer.dialog),
            Some(Dialog::Search { .. })
        );
        match event {
            EditorEvent::Submit => self.accept(&Accept, window, cx),
            EditorEvent::Cancel => self.close_dialog(window, cx),
            EditorEvent::Changed if searching => {
                self.set_search_index(0, cx);
                if let Some(Layer {
                    dialog: Dialog::Search { scroll, .. },
                    ..
                }) = &self.dialog
                {
                    // Filtering invalidates the previous results' measured bounds.
                    scroll.set_offset(gpui::point(px(0.), px(0.)));
                }
                self.run_search_preview(cx);
            }
            EditorEvent::Up if searching => self.step_search(-1, cx),
            EditorEvent::Down if searching => self.step_search(1, cx),
            EditorEvent::Changed => {
                self.uppercase_prefix(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn next_field(
        &mut self,
        _: &NextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_field(1, window, cx);
    }

    pub(crate) fn previous_field(
        &mut self,
        _: &PreviousField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_field(-1, window, cx);
    }

    fn cycle_field(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let fields = match self.dialog.as_ref() {
            Some(layer) => layer.fields.clone(),
            None => self.composer_fields(cx),
        };
        if fields.is_empty() {
            return;
        }
        let at = fields.iter().position(|field| field.is_focused(window));
        let count = fields.len() as isize;
        let next = at.map_or(0, |at| (at as isize + delta).rem_euclid(count));
        window.focus(&fields[next as usize], cx);
        cx.notify();
    }

    pub(crate) fn open_search(
        &mut self,
        _: &OpenSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = "Search tasks, notes, labels and refs";
        let input = cx.new(|cx| Editor::new(Mode::Single, placeholder, window, cx).emit_arrows());
        let dialog = Dialog::Search {
            input: input.clone(),
            all: self.session.snapshot.view.project.is_none(),
            deleted: self.session.snapshot.view.deleted != crate::adapter::DeletedFilter::Hide,
            index: 0,
            scroll: ScrollHandle::new(),
        };
        self.session.search = Default::default();
        self.open_dialog(dialog, vec![input], window, cx);
    }

    fn search_input(&self, cx: &Context<Self>) -> Option<SearchInput> {
        match &self.dialog.as_ref()?.dialog {
            Dialog::Search {
                input,
                all,
                deleted,
                ..
            } => Some(SearchInput {
                text: input.read(cx).text().trim().to_string(),
                project: (!all)
                    .then(|| self.session.snapshot.view.project.clone())
                    .flatten(),
                deleted: *deleted,
            }),
            _ => None,
        }
    }

    fn run_search_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.search_input(cx) {
            if input.text.is_empty() {
                self.session.search = Default::default();
            } else {
                self.session.search_preview(input);
            }
        }
        cx.notify();
    }

    /// What the list holds with nothing typed: the tasks touched last.
    fn recent_hits(&self) -> Vec<SearchHit> {
        let catalog = self.session.catalog.iter().take(6);
        catalog
            .map(|task| SearchHit {
                id: task.id.clone(),
                reference: task.reference.clone(),
                project: task.project.clone(),
                project_key: task.project_key.clone(),
                title: task.title.clone(),
                status: task.status,
                snippet: None,
                deleted: false,
                is_epic: task.is_epic,
            })
            .collect()
    }

    fn search_hits(&self, cx: &Context<Self>) -> Vec<SearchHit> {
        match self.search_input(cx) {
            Some(input) if input.text.is_empty() => self.recent_hits(),
            _ => self.session.search.hits.clone(),
        }
    }

    fn set_search_index(&mut self, next: usize, cx: &mut Context<Self>) {
        if let Some(Layer {
            dialog: Dialog::Search { index, .. },
            ..
        }) = self.dialog.as_mut()
        {
            *index = next;
        }
        cx.notify();
    }

    fn step_search(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.search_hits(cx).len() as isize;
        if let Some(Layer {
            dialog: Dialog::Search { index, .. },
            ..
        }) = self.dialog.as_ref()
        {
            let next = (*index as isize + delta).clamp(0, (count - 1).max(0));
            self.set_search_index(next as usize, cx);
            if let Some(Layer {
                dialog: Dialog::Search { input, scroll, .. },
                ..
            }) = &self.dialog
            {
                // Recent results have a heading before the first selectable row.
                let heading = usize::from(input.read(cx).text().trim().is_empty());
                if next == 0 {
                    scroll.set_offset(gpui::point(px(0.), px(0.)));
                } else {
                    reveal_item(scroll, next as usize + heading, px(SEARCH_LIST_PADDING));
                }
            }
        }
    }

    fn open_hit(&mut self, id: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.search_input(cx).filter(|input| !input.text.is_empty()) {
            self.session.search_accepted(&input.text);
        }
        self.close_dialog(window, cx);
        self.open_task(id, window, cx);
    }

    pub(crate) fn toggle_search_scope(
        &mut self,
        _: &ToggleSearchScope,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.snapshot.view.project.is_none() {
            return;
        }
        if let Some(Layer {
            dialog: Dialog::Search { all, .. },
            ..
        }) = self.dialog.as_mut()
        {
            *all = !*all;
            self.run_search_preview(cx);
        }
    }

    fn toggle_search_deleted(&mut self, cx: &mut Context<Self>) {
        if let Some(Layer {
            dialog: Dialog::Search { deleted, .. },
            ..
        }) = self.dialog.as_mut()
        {
            *deleted = !*deleted;
            self.run_search_preview(cx);
        }
    }

    pub(crate) fn full_results(
        &mut self,
        _: &FullResults,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.search_input(cx).filter(|input| !input.text.is_empty()) else {
            return;
        };
        self.session.search_accepted(&input.text);
        self.close_dialog(window, cx);
        self.switch_view(Command::Search(input), window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_prompt(
        &mut self,
        title: String,
        label: &'static str,
        hint: &'static str,
        value: &str,
        save: SavePromptText,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_prompt_in(Mode::Single, title, label, hint, value, save, window, cx);
    }

    /// A prompt in a given editor mode, for text that may span lines.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_prompt_in(
        &mut self,
        mode: Mode,
        title: String,
        label: &'static str,
        hint: &'static str,
        value: &str,
        save: SavePromptText,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut editor = Editor::new(mode, "", window, cx);
            editor.set_text(value, cx);
            editor.select_all_text(cx);
            editor
        });
        let dialog = Dialog::Prompt {
            title,
            label,
            hint,
            input: input.clone(),
            save,
        };
        self.open_dialog(dialog, vec![input], window, cx);
    }

    pub(crate) fn rename_task(
        &mut self,
        _: &RenameTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.session.target_row().cloned() else {
            return;
        };
        let id = row.id.clone();
        let save: SavePromptText = Rc::new(move |root, text, _, _| {
            if !text.trim().is_empty() {
                root.session.set_title(&id, text.trim().to_string());
            }
        });
        let title = format!("Rename {}", row.reference);
        self.open_prompt(title, "Title", "", &row.title, save, window, cx);
    }

    /// Sets a metadata field on the target. An empty value is still a value.
    pub(crate) fn open_meta(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.session.target().cloned() else {
            return;
        };
        let detail = self.session.detail.as_ref();
        let value = detail
            .and_then(|detail| detail.metadata.iter().find(|(field, _)| *field == key))
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let hint =
            "An empty value is different from no value. Use Remove in the task to clear the field.";
        let field = key.clone();
        let save: SavePromptText = Rc::new(move |root, text, _, _| {
            let command = Command::SetMetadata(id.clone(), field.clone(), Some(text));
            root.session.write(command);
        });
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

    pub(crate) fn open_image(&mut self, image: Image, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.session.target().cloned() else {
            return;
        };
        let alt = cx.new(|cx| {
            let mut editor = Editor::new(Mode::Single, "Describe this image", window, cx);
            editor.set_text(&image.alt, cx);
            editor
        });
        self.open_dialog(
            Dialog::Image {
                id,
                image,
                alt: alt.clone(),
            },
            vec![alt],
            window,
            cx,
        );
    }

    fn close_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Layer {
            dialog: Dialog::Image { id, image, alt },
            ..
        }) = self.dialog.as_ref()
        {
            let text = alt.read(cx).text().to_string();
            if text != image.alt {
                let (id, attachment) = (id.clone(), image.id.clone());
                self.session
                    .write(Command::SetImageAlt(id, attachment, text));
            }
        }
        self.close_dialog(window, cx);
    }

    pub(crate) fn open_about(
        &mut self,
        _: &OpenAbout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_dialog(Dialog::About, Vec::new(), window, cx);
    }

    pub(crate) fn open_help(&mut self, _: &OpenHelp, window: &mut Window, cx: &mut Context<Self>) {
        self.open_dialog(Dialog::Help, Vec::new(), window, cx);
    }

    /// Compares a description being edited with the one that arrived.
    pub(crate) fn open_incoming(
        &mut self,
        id: TaskId,
        mine: String,
        theirs: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_dialog(
            Dialog::Incoming { id, mine, theirs },
            Vec::new(),
            window,
            cx,
        );
    }

    pub(crate) fn accept(&mut self, _: &Accept, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layer) = self.dialog.as_ref() else {
            return;
        };
        match &layer.dialog {
            Dialog::Search { index, .. } => {
                let hit = self.search_hits(cx).get(*index).cloned();
                if let Some(hit) = hit {
                    self.open_hit(hit.id, window, cx);
                }
            }
            Dialog::Prompt { input, save, .. } => {
                let (text, save) = (input.read(cx).text().to_string(), save.clone());
                self.close_dialog(window, cx);
                save(self, text, window, cx);
            }
            Dialog::Image { .. } => self.close_image(window, cx),
            Dialog::Sync { .. } => self.sync_accept(window, cx),
            Dialog::Project { .. } => self.save_project(window, cx),
            Dialog::Help
            | Dialog::About
            | Dialog::Confirm { .. }
            | Dialog::Incoming { .. }
            | Dialog::Stop { .. }
            | Dialog::Resolve { .. }
            | Dialog::Gist { .. }
            | Dialog::Update { .. } => self.close_dialog(window, cx),
        }
    }

    fn render_search(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Layer {
            dialog:
                Dialog::Search {
                    input,
                    all,
                    deleted,
                    index,
                    scroll,
                },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        let text = input.read(cx).text().trim().to_string();
        let scope = self.session.snapshot.view.project.clone();
        let scope_name = scope
            .as_ref()
            .and_then(|key| {
                let projects = &self.session.snapshot.projects;
                projects.iter().find(|project| project.key == *key)
            })
            .map(|project| project.name.clone());
        let hits = self.search_hits(cx);
        let results = &self.session.search;
        let mut list = div()
            .id("search-list")
            .h(px(SEARCH_LIST_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(scroll)
            .p(px(SEARCH_LIST_PADDING));
        if text.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text(Text::Caption)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .child("Recent"),
            );
        }
        for (ix, hit) in hits.iter().enumerate() {
            list = list.child(self.search_row(ix, hit, ix == *index, &text, cx, t));
        }
        if hits.is_empty() && !text.is_empty() {
            let other = results.elsewhere;
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(48.))
                    .text_color(t.muted)
                    .child(format!("No matches for “{text}”"))
                    .when(other > 0, |none| {
                        none.child(
                            line_btn(
                                "search-elsewhere",
                                format!("Show {other} in other projects"),
                                t,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.toggle_search_scope(&ToggleSearchScope, window, cx);
                                },
                            )),
                        )
                    })
                    .when(other == 0 && !*deleted, |none| {
                        none.child(
                            small(Text::Secondary)
                                .text_color(t.faint)
                                .child("Deleted tasks are hidden."),
                        )
                    }),
            );
        }
        let workspace = &self.session.snapshot.workspace;
        let where_ = if *all {
            format!("{workspace}, all projects")
        } else {
            format!("{workspace}, {}", scope_name.clone().unwrap_or_default())
        };
        let count = if text.is_empty() {
            "Includes done and canceled tasks".to_string()
        } else {
            format!("{} results in {where_}", results.total)
        };
        vec![
            div()
                .h(px(52.))
                .px(px(16.))
                .flex()
                .items_center()
                .gap(px(10.))
                .border_b_1()
                .border_color(t.line)
                .child(icon("icons/ic-search.svg", t.faint))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text(Text::Title)
                        .text_color(t.ink)
                        .child(input.clone()),
                )
                .when_some(scope_name, |top, name| {
                    let label = if *all {
                        "All projects".to_string()
                    } else {
                        name
                    };
                    top.child(
                        btn("search-scope", label.clone(), t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_search_scope(&ToggleSearchScope, window, cx);
                            }))
                            .child(label),
                    )
                })
                .into_any_element(),
            list.into_any_element(),
            div()
                .h(px(40.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(6.))
                .border_t_1()
                .border_color(t.line)
                .child(
                    small(Text::Secondary)
                        .flex_1()
                        .truncate()
                        .text_color(t.faint)
                        .child(count),
                )
                .child(
                    btn(
                        "search-deleted",
                        if *deleted {
                            "Hide deleted"
                        } else {
                            "Include deleted"
                        },
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_search_deleted(cx)))
                    .child(if *deleted {
                        "Hide deleted"
                    } else {
                        "Include deleted"
                    }),
                )
                .child(
                    btn("search-full", "All results", t)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.full_results(&FullResults, window, cx);
                        }))
                        .child("All results")
                        .child(kbd("⇧↩", t.faint)),
                )
                .into_any_element(),
        ]
    }

    fn search_row(
        &self,
        ix: usize,
        hit: &SearchHit,
        on: bool,
        text: &str,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        let id = hit.id.clone();
        let status = status_name(hit.status);
        let detail = hit.snippet.as_ref().map_or(status.to_string(), |snippet| {
            format!("{status} · {snippet}")
        });
        // Matches are weight and underline rather than a fill, so they never
        // read as a second selection.
        let highlight = HighlightStyle {
            font_weight: Some(FontWeight::SEMIBOLD),
            underline: Some(UnderlineStyle {
                thickness: px(1.5),
                color: Some(t.accent.opacity(0.7)),
                wavy: false,
            }),
            ..HighlightStyle::default()
        };
        let detail_highlight = HighlightStyle {
            color: Some(t.body),
            ..highlight
        };
        let marked = |line: &str, highlight: HighlightStyle| -> StyledText {
            let lower = line.to_lowercase();
            let ranges = text
                .split_whitespace()
                .flat_map(|word| {
                    let word = word.to_lowercase();
                    let found: Vec<_> = lower
                        .match_indices(&word)
                        .map(|(at, _)| (at..at + word.len(), highlight))
                        .collect();
                    found
                })
                .filter(|(range, _)| {
                    line.len() == lower.len()
                        && line.is_char_boundary(range.start)
                        && line.is_char_boundary(range.end)
                })
                .collect::<Vec<_>>();
            let mut ranges = ranges;
            ranges.sort_by_key(|(range, _)| range.start);
            ranges.dedup_by(|later, earlier| later.0.start < earlier.0.end);
            StyledText::new(SharedString::from(line.to_string())).with_highlights(ranges)
        };
        div()
            .id(("search-row", ix))
            .role(Role::ListBoxOption)
            .aria_label(format!("{} {}", hit.reference, hit.title))
            .aria_selected(on)
            .h(px(SEARCH_ROW_HEIGHT))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(RADIUS_CONTROL))
            .when(on, |row| row.bg(t.pick()))
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                let hovered = match this.dialog.as_ref() {
                    Some(Layer {
                        dialog: Dialog::Search { index, .. },
                        ..
                    }) => *index != ix,
                    _ => false,
                };
                if hovered {
                    this.set_search_index(ix, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_hit(id.clone(), window, cx);
            }))
            .child(status_glyph(hit.status, t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(t.ink)
                                    .when(hit.deleted, |title| title.line_through())
                                    .child(marked(&hit.title, highlight)),
                            )
                            .when(hit.is_epic, |line| {
                                line.child(epic_star(hit.status.is_terminal() || hit.deleted, t))
                            }),
                    )
                    .child(
                        // The ref leads the line, so the project sits under the
                        // start of the title.
                        small(Text::Secondary)
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_color(if on { t.body } else { t.muted })
                            .child(ref_label(&hit.reference, &hit.project_key, None, t))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(marked(&detail, detail_highlight)),
                            ),
                    ),
            )
            .when(hit.deleted, |row| {
                row.child(
                    small(Text::Secondary)
                        .flex_none()
                        .text_color(t.bad)
                        .child("Deleted"),
                )
            })
    }

    /// Prefixes are capitals, so typing shows them as they will save.
    fn uppercase_prefix(&mut self, cx: &mut Context<Self>) {
        if let Some(Layer {
            dialog: Dialog::Project { prefix, .. },
            ..
        }) = self.dialog.as_ref()
        {
            let upper = prefix.read(cx).text().to_uppercase();
            if upper != prefix.read(cx).text() {
                prefix.update(cx, |editor, cx| editor.set_text(&upper, cx));
            }
        }
    }

    pub(crate) fn field_label(text: &'static str, t: &Theme) -> impl IntoElement + use<> {
        small(Text::Secondary)
            .mt(px(10.))
            .mb(px(4.))
            .text_color(t.faint)
            .child(text)
    }

    pub(crate) fn field_box(editor: &Entity<Editor>, t: &Theme) -> impl IntoElement + use<> {
        div()
            .h(px(30.))
            .px(px(9.))
            .flex()
            .items_center()
            .rounded(px(RADIUS_CONTROL))
            .bg(t.page)
            .shadow(vec![t.inset_ring(t.line)])
            .text_color(t.ink)
            .child(editor.clone())
    }

    pub(crate) fn heading(text: String, t: &Theme) -> impl IntoElement + use<> {
        div()
            .px(px(18.))
            .pt(px(14.))
            .text(Text::Title)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.ink)
            .child(text)
    }

    pub(crate) fn footer(t: &Theme) -> gpui::Div {
        div()
            .flex()
            .justify_end()
            .gap(px(6.))
            .px(px(14.))
            .py(px(10.))
            .mt(px(4.))
            .border_t_1()
            .border_color(t.line)
    }

    pub(crate) fn cancel_and_accept(
        &self,
        label: &'static str,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        Self::footer(t)
            .child(
                line_btn("dialog-cancel", "Cancel", t)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            )
            .child(
                fill_btn("dialog-accept", label, t)
                    .on_click(cx.listener(|this, _, window, cx| this.accept(&Accept, window, cx))),
            )
    }

    fn render_prompt(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Layer {
            dialog:
                Dialog::Prompt {
                    title,
                    label,
                    hint,
                    input,
                    ..
                },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        vec![
            Self::heading(title.clone(), t).into_any_element(),
            div()
                .px(px(18.))
                .pb(px(12.))
                .child(Self::field_label(label, t))
                .child(Self::field_box(input, t))
                .when(!hint.is_empty(), |body| {
                    body.child(
                        small(Text::Secondary)
                            .mt(px(8.))
                            .text_color(t.faint)
                            .child(*hint),
                    )
                })
                .into_any_element(),
            self.cancel_and_accept("Save", cx, t).into_any_element(),
        ]
    }

    fn render_image(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> Vec<AnyElement> {
        let Some(Layer {
            dialog: Dialog::Image { id, image, alt },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        let viewer = div()
            .h((viewport.height * 0.78 - px(IMAGE_DIALOG_CHROME)).max(px(300.)))
            .rounded(px(RADIUS_SURFACE))
            .bg(t.raised)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .text_color(t.faint);
        let viewer = match &image.preview {
            Some(path) => viewer.child(
                img(path.clone())
                    .size_full()
                    .rounded(px(RADIUS_SURFACE))
                    .object_fit(ObjectFit::ScaleDown),
            ),
            None => viewer.child("Not on this device"),
        };
        let (save_id, save_name) = (image.id.clone(), image.name.clone());
        let (task, attachment) = (id.clone(), image.id.clone());
        vec![
            div()
                .flex()
                .items_center()
                .pr(px(10.))
                .child(div().flex_1().child(Self::heading(image.name.clone(), t)))
                .child(
                    btn("image-close", "Close", t)
                        .mt(px(8.))
                        .on_click(cx.listener(|this, _, window, cx| this.close_image(window, cx)))
                        .child("Close"),
                )
                .into_any_element(),
            div()
                .px(px(18.))
                .pt(px(10.))
                .pb(px(12.))
                .child(viewer)
                .child(Self::field_label("Description", t))
                .child(Self::field_box(alt, t))
                .into_any_element(),
            Self::footer(t)
                .child(line_btn("image-save", "Save copy", t).on_click(cx.listener(
                    move |this, _, _, cx| this.save_image_copy(save_id.clone(), &save_name, cx),
                )))
                .child(line_btn("image-remove", "Remove", t).on_click(cx.listener(
                    move |this, _, window, cx| {
                        let command = Command::DeleteImage(task.clone(), attachment.clone());
                        this.session.write(command);
                        this.close_dialog(window, cx);
                    },
                )))
                .child(
                    fill_btn("image-done", "Done", t)
                        .on_click(cx.listener(|this, _, window, cx| this.close_image(window, cx))),
                )
                .into_any_element(),
        ]
    }

    fn render_about(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let version = format!("Version {}", crate::updates::version(cx));
        let links = ABOUT_LINKS.iter().map(|(label, url)| {
            bare_btn(*label, *label, t)
                .role(Role::Link)
                .hover({
                    let raised = t.raised;
                    move |style| style.bg(raised)
                })
                .on_click(move |_, _, cx| cx.open_url(url))
                .child(*label)
        });
        vec![
            div()
                .relative()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(31.))
                .px(px(22.))
                .pb(px(18.))
                .child(
                    div().absolute().top(px(8.)).right(px(8.)).child(
                        btn("about-close", "Close", t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_dialog(window, cx);
                            }))
                            .child("Close"),
                    ),
                )
                .child(
                    gpui::img(if t.dark {
                        "logo-dark.svg"
                    } else {
                        "logo-light.svg"
                    })
                    .size(px(64.))
                    .mb(px(12.)),
                )
                .child(
                    div()
                        .text(Text::Title)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.ink)
                        .child("Aven"),
                )
                .child(
                    div()
                        .id("about-version")
                        .role(Role::Button)
                        .aria_label("Copy version")
                        .mt(px(7.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(RADIUS_CHIP))
                        .text(Text::Secondary)
                        .text_color(t.muted)
                        .hover({
                            let (raised, ink) = (t.raised, t.ink);
                            move |style| style.bg(raised).text_color(ink)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let version = format!("Aven {}", crate::updates::version(cx));
                            cx.write_to_clipboard(ClipboardItem::new_string(version));
                            this.session.show_toast(Toast::done("Version copied"));
                            cx.notify();
                        }))
                        .child(version),
                )
                .child(
                    div()
                        .mt(px(12.))
                        .text_color(t.body)
                        .child("Local-first task manager"),
                )
                .child(div().flex().gap(px(2.)).mt(px(14.)).children(links))
                .child(
                    div()
                        .mt(px(12.))
                        .text(Text::Secondary)
                        .text_color(t.faint)
                        .child("© 2026 Raine Virta. MIT License."),
                )
                .into_any_element(),
        ]
    }

    fn render_help(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let mut columns = div().flex().flex_wrap().px(px(10.)).pb(px(14.));
        for (family, keys) in HELP {
            let mut column = div().w(relative_third()).px(px(8.)).mt(px(12.)).child(
                div()
                    .mb(px(4.))
                    .text(Text::Secondary)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.muted)
                    .child(family),
            );
            for (text, key) in keys {
                column = column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_h(px(22.))
                        .text(Text::Secondary)
                        .child(div().flex_none().text_color(t.body).child(*text))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(MONO)
                                .text_right()
                                .text_color(t.accent)
                                .child(*key),
                        ),
                );
            }
            columns = columns.child(column);
        }
        vec![
            div()
                .flex()
                .items_center()
                .pr(px(10.))
                .pb(px(12.))
                .child(
                    div()
                        .flex_1()
                        .child(Self::heading("Keyboard shortcuts".to_string(), t)),
                )
                .child(
                    btn("help-close", "Close", t)
                        .mt(px(8.))
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)))
                        .child("Close"),
                )
                .into_any_element(),
            columns.into_any_element(),
        ]
    }

    fn render_incoming(&self, cx: &mut Context<Self>, t: &Theme) -> Vec<AnyElement> {
        let Some(Layer {
            dialog: Dialog::Incoming { id, mine, theirs },
            ..
        }) = self.dialog.as_ref()
        else {
            return Vec::new();
        };
        let side = |heading: &'static str, text: &str| {
            div()
                .flex_1()
                .min_w_0()
                .child(Self::field_label(heading, t))
                .child(
                    div()
                        .id(heading)
                        .h(px(240.))
                        .overflow_y_scroll()
                        .p(px(10.))
                        .rounded(px(RADIUS_CONTROL))
                        .bg(t.raised)
                        .text_color(t.body)
                        .child(text.to_string()),
                )
        };
        let (task, text) = (id.clone(), mine.clone());
        vec![
            Self::heading("Review the incoming description".to_string(), t).into_any_element(),
            div()
                .px(px(18.))
                .pb(px(12.))
                .flex()
                .gap(px(12.))
                .child(side("Your draft", mine))
                .child(side("Incoming", theirs))
                .into_any_element(),
            Self::footer(t)
                .child(
                    line_btn("incoming-take", "Use incoming", t).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.close_dialog(window, cx);
                            this.cancel_description(window, cx);
                        },
                    )),
                )
                .child(
                    fill_btn("incoming-keep", "Keep my draft", t).on_click(cx.listener(
                        move |this, _, window, cx| {
                            let command = Command::SetDescription(task.clone(), text.clone());
                            this.session.write(command);
                            this.close_dialog(window, cx);
                            this.finish_description(window, cx);
                        },
                    )),
                )
                .into_any_element(),
        ]
    }

    fn scroll_help(&self, direction: f32, page: bool, cx: &mut Context<Self>) {
        let Some(layer) = &self.dialog else {
            return;
        };
        let scroll = &layer.scroll;
        let offset = scroll.offset();
        let y = help_scroll_offset(
            offset.y,
            scroll.max_offset().y,
            scroll.bounds().size.height,
            direction,
            page,
        );
        scroll.set_offset(gpui::point(offset.x, y));
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn render_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let layer = self.dialog.as_ref()?;
        let t = theme(cx);
        let viewport = window.viewport_size();
        let (name, width, content, context) = match &layer.dialog {
            Dialog::Search { .. } => (
                "Search",
                SEARCH_WIDTH,
                self.render_search(cx, &t),
                "Dialog Search",
            ),
            Dialog::Prompt { .. } => ("Edit", NARROW_WIDTH, self.render_prompt(cx, &t), "Dialog"),
            Dialog::Stop { .. } => ("Stop", NARROW_WIDTH, self.render_stop(cx, &t), "Dialog"),
            Dialog::Gist { .. } => ("Gist", NARROW_WIDTH, self.render_gist(cx, &t), "Dialog"),
            Dialog::Sync { .. } => {
                let width = if self.session.sync_page == SyncPage::Home
                    && self.session.snapshot.sync.phase != SyncPhase::Local
                {
                    DIALOG_WIDTH
                } else {
                    NARROW_WIDTH
                };
                ("Sync", width, self.render_sync(cx, &t), "Dialog")
            }
            Dialog::Confirm { .. } => (
                "Confirm",
                DIALOG_WIDTH,
                self.render_confirm(cx, &t),
                "Dialog",
            ),
            Dialog::Project { .. } => (
                "Project",
                DIALOG_WIDTH,
                self.render_project(cx, &t),
                "Dialog",
            ),
            Dialog::Resolve { .. } => (
                "Resolve conflict",
                DIALOG_WIDTH,
                self.render_resolve(cx, &t),
                "Dialog",
            ),
            Dialog::Image { .. } => (
                "Image",
                IMAGE_DIALOG_WIDTH,
                self.render_image(viewport, cx, &t),
                "Dialog",
            ),
            Dialog::About => (
                "About Aven",
                ABOUT_WIDTH,
                self.render_about(cx, &t),
                "Dialog",
            ),
            Dialog::Update { .. } => ("Update", NARROW_WIDTH, self.render_update(cx, &t), "Dialog"),
            Dialog::Help => (
                "Keyboard shortcuts",
                WIDE_WIDTH,
                self.render_help(cx, &t),
                "Dialog Help",
            ),
            Dialog::Incoming { .. } => (
                "Incoming description",
                WIDE_WIDTH,
                self.render_incoming(cx, &t),
                "Dialog",
            ),
        };
        let width = width.min(f32::from(viewport.width) - 32.);
        // Multi-part dialogs put their heading first. Keep it outside the
        // scrolling body; single-part dialogs scroll as a whole.
        let has_header = content.len() > 1;
        let mut content = content.into_iter();
        let header = has_header.then(|| content.next()).flatten();
        let scroll = layer.scroll.clone();
        let bar = t.bar;
        let body = div()
            .relative()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("dialog-body")
                    .min_h_0()
                    .pr(px(8.))
                    .overflow_y_scroll()
                    .track_scroll(&layer.scroll)
                    .child(div().children(content)),
            )
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        if let Some((top, height)) = dialog_scroll_thumb(
                            bounds.size.height,
                            scroll.bounds().size.height,
                            scroll.max_offset().y,
                            -scroll.offset().y,
                        ) {
                            window.paint_quad(gpui::fill(
                                gpui::Bounds::new(
                                    gpui::point(bounds.origin.x, bounds.origin.y + top),
                                    gpui::size(bounds.size.width, height),
                                ),
                                bar,
                            ));
                        }
                    },
                )
                .absolute()
                .right(px(3.))
                .top(px(4.))
                .bottom(px(4.))
                .w(px(4.)),
            );
        Some(
            div()
                .id("dialog-backdrop")
                .absolute()
                .inset_0()
                .bg(t.shade)
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_dialog(window, cx);
                    }),
                )
                .child(
                    div()
                        .id("dialog")
                        .role(Role::Dialog)
                        .aria_label(name)
                        .key_context(context)
                        .track_focus(&layer.container)
                        .on_action(cx.listener(Self::accept))
                        .when(matches!(layer.dialog, Dialog::Help), |dialog| {
                            dialog
                                .on_action(cx.listener(|this, _: &MoveDown, _, cx| {
                                    this.scroll_help(1., false, cx);
                                }))
                                .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                                    this.scroll_help(-1., false, cx);
                                }))
                                .on_action(cx.listener(|this, _: &HelpPageDown, _, cx| {
                                    this.scroll_help(1., true, cx);
                                }))
                                .on_action(cx.listener(|this, _: &HelpPageUp, _, cx| {
                                    this.scroll_help(-1., true, cx);
                                }))
                        })
                        .on_action(cx.listener(Self::toggle_search_scope))
                        .on_action(cx.listener(Self::full_results))
                        .absolute()
                        .left((viewport.width - px(width)) / 2.)
                        .top(viewport.height * 0.11)
                        .w(px(width))
                        .max_h(viewport.height * 0.78)
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .rounded(px(RADIUS_SURFACE))
                        .bg(t.pop)
                        .shadow(t.dlg_shadow())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .when_some(header, |dialog, header| {
                            dialog.child(div().flex_none().child(header))
                        })
                        .child(body),
                ),
        )
    }
}

fn relative_third() -> gpui::Length {
    gpui::relative(1. / 3.).into()
}

/// Thumb geometry is calculated at paint time so wheel scrolling and resizing
/// update the indicator without waiting for another view render.
fn dialog_scroll_thumb(
    track: gpui::Pixels,
    viewport: gpui::Pixels,
    max: gpui::Pixels,
    offset: gpui::Pixels,
) -> Option<(gpui::Pixels, gpui::Pixels)> {
    if max <= px(0.) || track <= px(0.) || viewport <= px(0.) {
        return None;
    }
    let height = (track * (viewport / (viewport + max)))
        .max(px(24.))
        .min(track);
    let top = (track - height) * (offset / max).clamp(0., 1.);
    Some((top, height))
}

fn help_scroll_offset(
    current: gpui::Pixels,
    max: gpui::Pixels,
    viewport: gpui::Pixels,
    direction: f32,
    page: bool,
) -> gpui::Pixels {
    let row = px(22.);
    let step = if page { (viewport - row).max(row) } else { row };
    (current - step * direction).clamp(-max, px(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_indicator_tracks_overflow_and_clamps_to_its_track() {
        assert_eq!(
            dialog_scroll_thumb(px(100.), px(100.), px(0.), px(0.)),
            None
        );
        assert_eq!(
            dialog_scroll_thumb(px(0.), px(100.), px(100.), px(0.)),
            None
        );
        assert_eq!(
            dialog_scroll_thumb(px(100.), px(100.), px(100.), px(0.)),
            Some((px(0.), px(50.)))
        );
        assert_eq!(
            dialog_scroll_thumb(px(100.), px(100.), px(100.), px(50.)),
            Some((px(25.), px(50.)))
        );
        assert_eq!(
            dialog_scroll_thumb(px(100.), px(100.), px(100.), px(200.)),
            Some((px(50.), px(50.)))
        );
        assert_eq!(
            dialog_scroll_thumb(px(10.), px(10.), px(1000.), px(1000.)),
            Some((px(0.), px(10.)))
        );
    }

    #[test]
    fn help_scrolls_by_rows_or_pages_with_one_row_of_overlap() {
        assert_eq!(
            help_scroll_offset(px(0.), px(900.), px(300.), 1., false),
            px(-22.)
        );
        assert_eq!(
            help_scroll_offset(px(-44.), px(900.), px(300.), -1., false),
            px(-22.)
        );
        assert_eq!(
            help_scroll_offset(px(0.), px(900.), px(300.), 1., true),
            px(-278.)
        );
        assert_eq!(
            help_scroll_offset(px(-556.), px(900.), px(300.), -1., true),
            px(-278.)
        );
    }

    #[test]
    fn help_scrolling_stays_within_the_body() {
        assert_eq!(
            help_scroll_offset(px(-890.), px(900.), px(300.), 1., true),
            px(-900.)
        );
        assert_eq!(
            help_scroll_offset(px(-10.), px(900.), px(300.), -1., true),
            px(0.)
        );
        assert_eq!(
            help_scroll_offset(px(0.), px(0.), px(300.), 1., true),
            px(0.)
        );
        assert_eq!(
            help_scroll_offset(px(0.), px(100.), px(10.), 1., true),
            px(-22.)
        );
    }
}

//! What the app's commands do: the handlers behind keys, menus and the palette.
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use aven_core::choices::TaskStatus;
use aven_core::ids::TaskId;
use aven_host::store::TaskQuery;
use gpui::{Bounds, ClipboardItem, Context, Pixels, Window, size};

use crate::actions::*;
use crate::adapter::{
    Bulk, Command, DeletedFilter, GroupBy, MetaFilter, PendingImage, TaskRow, ViewInfo, ViewOrder,
};
use crate::input::{self, PastedImage, clipboard_image};
use crate::kit::*;
use crate::overlays::{Entry, Mark, MenuOptions};
use crate::shell::Root;
use crate::toast::Toast;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterKind {
    Priority,
    Label,
    Metadata,
}

const ORDERS: [(ViewOrder, &str, char); 7] = [
    (ViewOrder::Priority, "Priority", 'p'),
    (ViewOrder::Due, "Due date", 'd'),
    (ViewOrder::Available, "Availability", 'a'),
    (ViewOrder::Created, "Recently created", 'c'),
    (ViewOrder::Updated, "Recently updated", 'u'),
    (ViewOrder::Project, "Project", 'j'),
    (ViewOrder::Title, "Title", 't'),
];

/// A choice menu's title, saying how many tasks a letter will change.
fn choice_title(title: &str, marked: usize) -> String {
    if marked > 0 {
        format!("{title} · {marked} marked")
    } else {
        title.to_string()
    }
}

/// What the order button says about the list.
pub fn order_label(view: &ViewInfo) -> String {
    if view.query == TaskQuery::Queue {
        return "Order".to_string();
    }
    let name = match view.order {
        ViewOrder::Priority => "Priority",
        ViewOrder::Due => "Due",
        ViewOrder::Available => "Available",
        ViewOrder::Created => "Created",
        ViewOrder::Updated => "Updated",
        ViewOrder::Project => "Project",
        ViewOrder::Title => "Title",
    };
    let arrow = if view.descending { "↓" } else { "↑" };
    format!("{name} {arrow}")
}
const GROUPS: [(GroupBy, &str); 4] = [
    (GroupBy::Status, "Status"),
    (GroupBy::Project, "Project"),
    (GroupBy::Priority, "Priority"),
    (GroupBy::None, "No grouping"),
];
/// Lanes in the order `<` and `>` walk them.
const LANES: [TaskStatus; 5] = [
    TaskStatus::Inbox,
    TaskStatus::Backlog,
    TaskStatus::Todo,
    TaskStatus::Active,
    TaskStatus::Done,
];
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "heic", "tiff"];

pub fn view_name(query: TaskQuery) -> &'static str {
    match query {
        TaskQuery::Search => "Search results",
        _ => VIEWS
            .iter()
            .find(|view| view.0 == query)
            .map_or("Tasks", |view| view.1),
    }
}

pub(crate) fn is_image(path: &Path) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str());
    extension.is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
}

/// Reads an image from disk, by the name it will keep. Other files read as nothing.
pub fn read_image(path: &Path) -> Option<PendingImage> {
    if !is_image(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    Some(storable(name, bytes))
}

/// The image a paste carries.
pub fn pasted_image(pasted: PastedImage) -> Option<PendingImage> {
    match pasted {
        PastedImage::Path(path) => read_image(&path),
        PastedImage::Bytes(image) => Some(storable("pasted.png".to_string(), image.bytes)),
    }
}

/// Aven stores PNG, JPEG, GIF and WebP. Other formats that decode here, such
/// as the TIFF many macOS apps copy, become PNG; the rest go through as they
/// are, so the store can say what's wrong with them.
fn storable(name: String, bytes: Vec<u8>) -> PendingImage {
    use image::ImageFormat;
    let stored = matches!(
        image::guess_format(&bytes),
        Ok(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP)
    );
    match (!stored).then(|| as_png(&bytes)).flatten() {
        Some(png) => PendingImage {
            name: Path::new(&name)
                .with_extension("png")
                .to_string_lossy()
                .to_string(),
            bytes: Arc::new(png),
        },
        None => PendingImage {
            name,
            bytes: Arc::new(bytes),
        },
    }
}

fn as_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let picture = image::load_from_memory(bytes).ok()?;
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

impl Root {
    /// The epic's focused child takes task keys ahead of the epic itself.
    pub(crate) fn target_id(&self) -> Option<TaskId> {
        self.session
            .child
            .clone()
            .or(self.session.target().cloned())
    }

    /// The rows of the marks that take the next write, if any do.
    pub(crate) fn marked_rows(&self) -> Vec<TaskRow> {
        let targets = self.session.bulk_targets().unwrap_or_default();
        let rows = targets
            .iter()
            .filter_map(|id| self.session.snapshot.row(id));
        rows.cloned().collect()
    }

    fn target_row(&self) -> Option<TaskRow> {
        match &self.session.child {
            Some(child) => self.session.snapshot.row(child).cloned(),
            None => self.session.target_row().cloned(),
        }
    }

    /// Where a menu about the target opens, unless a control asked for it.
    pub(crate) fn anchor(&self, key: &'static str) -> Bounds<Pixels> {
        let known = self.anchors.borrow().get(key).copied();
        let details = self.details_open || self.session.expanded;
        known
            .filter(|_| {
                details
                    || !matches!(
                        key,
                        "status" | "priority" | "labels" | "project" | "available" | "due"
                    )
            })
            .unwrap_or_else(|| self.selected_row_bounds())
    }

    /// Leaves what the task surfaces were doing for a new place in the list.
    pub(crate) fn leave_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_description(cx);
        self.commit_title(cx);
        self.keep_note_draft(cx);
        window.focus(&self.focus, cx);
    }

    pub(crate) fn switch_view(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_task(window, cx);
        self.session.switch_view(command);
        self.load_selection(window, cx);
        self.list_to_top();
        cx.notify();
    }

    pub(crate) fn show_view(
        &mut self,
        action: &ShowView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.0 == TaskQuery::Search && self.session.snapshot.view.search.is_none() {
            self.session.show_toast(Toast::info("No search results"));
            cx.notify();
            return;
        }
        self.switch_view(Command::ShowView(action.0), window, cx);
    }

    pub(crate) fn scope_to(
        &mut self,
        project: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_view(Command::ShowProject(project), window, cx);
    }

    pub(crate) fn all_projects(
        &mut self,
        _: &AllProjects,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scope_to(None, window, cx);
    }

    pub(crate) fn pick_project(
        &mut self,
        _: &PickProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut entries = vec![Entry::new("All projects", |root, window, cx| {
            root.scope_to(None, window, cx);
        })];
        for project in &self.session.snapshot.projects {
            let key = project.key.clone();
            entries.push(
                Entry::new(project.name.clone(), move |root, window, cx| {
                    root.scope_to(Some(key.clone()), window, cx);
                })
                .hint(project.prefix.clone())
                .mark(Mark::Project(project.key.clone())),
            );
        }
        let anchor = self.anchors.borrow().get("scope").copied();
        let options = MenuOptions {
            wide: anchor.is_none(),
            ..MenuOptions::default()
        };
        self.open_menu(
            entries,
            "Choose project scope",
            anchor.unwrap_or_default(),
            options,
            window,
            cx,
        );
    }

    pub(crate) fn pick_workspace(
        &mut self,
        _: &PickWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self
            .session
            .snapshot
            .workspaces
            .iter()
            .map(|workspace| {
                let key = workspace.key.clone();
                Entry::new(workspace.name.clone(), move |root, window, cx| {
                    root.switch_view(Command::SwitchWorkspace(key.clone()), window, cx);
                })
            })
            .collect();
        let anchor = self.anchors.borrow().get("workspace").copied();
        let options = MenuOptions {
            wide: anchor.is_none(),
            ..MenuOptions::default()
        };
        self.open_menu(
            entries,
            "Switch workspace",
            anchor.unwrap_or_default(),
            options,
            window,
            cx,
        );
    }

    pub(crate) fn toggle_layout(
        &mut self,
        _: &ToggleLayout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = &self.session.snapshot.view;
        if crate::adapter::supports_columns(view.query) {
            let on = !view.columns;
            self.session.write(Command::SetColumns(on));
        } else {
            let name = view_name(view.query);
            self.session
                .show_toast(Toast::info(format!("{name} is list only")));
        }
        cx.notify();
    }

    pub(crate) fn expand(&mut self, _: &Expand, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.snapshot.view.query == TaskQuery::Recurring {
            self.open_series_current(window, cx);
        } else if let Some(child) = self.session.child.clone() {
            self.follow(child, window, cx);
        } else if self.session.target().is_some() {
            self.session.clear_marks();
            self.session.expanded = true;
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    pub(crate) fn collapse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_task(window, cx);
        self.session.expanded = false;
        if self.session.peek.is_some() {
            let selected = self.session.selected.clone();
            self.session.selected = None;
            if let Some(id) = selected {
                self.session.select(id);
            }
            self.load_selection(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.marks.is_empty() {
            self.session.clear_marks();
            cx.notify();
        } else if !self.session.trail.is_empty() {
            self.trail_back(window, cx);
        } else if self.session.expanded {
            self.collapse(window, cx);
        } else if self.session.child.take().is_some() {
            cx.notify();
        } else if self.is_narrow() && self.details_open {
            self.details_open = false;
            cx.notify();
        } else if self.session.toast().is_some() {
            self.session.dismiss_toast();
            cx.notify();
        }
    }

    pub(crate) fn trail_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_task(window, cx);
        self.session.trail_back();
        self.load_selection(window, cx);
        cx.notify();
    }

    /// Opens a linked task beside the list, leaving the selection where it is.
    pub(crate) fn follow(&mut self, id: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_task(window, cx);
        if !self.session.expanded {
            self.details_open = true;
        }
        self.session.follow(id);
        self.load_selection(window, cx);
        cx.notify();
    }

    /// Opens a task from anywhere, keeping the list if the task is in it.
    pub fn open_task(&mut self, id: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_task(window, cx);
        if self.session.snapshot.row(&id).is_none() && !self.session.expanded {
            self.details_open = true;
        }
        self.session.open(id);
        self.load_selection(window, cx);
        self.reveal_selection();
        cx.notify();
    }

    pub(crate) fn jump_to_blocker(
        &mut self,
        _: &JumpToBlocker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blocker = self.session.detail.as_ref();
        match blocker.and_then(|detail| detail.blocked_by.first()) {
            Some(link) => self.follow(link.id.clone(), window, cx),
            None => {
                self.session.show_toast(Toast::info("No blocker"));
                cx.notify();
            }
        }
    }

    fn pick_status_at(
        &mut self,
        anchor: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.target_id() else {
            return;
        };
        let current = self.target_row().map(|row| row.status);
        let marked = self
            .session
            .bulk_targets()
            .map_or(0, |targets| targets.len());
        let entries = STATUSES
            .iter()
            .map(|(status, name)| {
                let (id, status) = (id.clone(), *status);
                Entry::new(*name, move |root, _, cx| {
                    if !root.session.bulk(Bulk::Status(status)) {
                        root.session.set_status(&id, status);
                    }
                    root.after_status_motion(cx);
                })
                .mark(Mark::Status(status))
                .key(status_key(status))
                .checked(marked == 0 && current == Some(status))
            })
            .collect();
        self.open_menu(
            entries,
            "Set status",
            anchor,
            MenuOptions {
                choice: Some(choice_title("Set status", marked)),
                ..MenuOptions::default()
            },
            window,
            cx,
        );
    }

    fn pick_priority_at(
        &mut self,
        anchor: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.target_id() else {
            return;
        };
        let current = self.target_row().map(|row| row.priority);
        let marked = self
            .session
            .bulk_targets()
            .map_or(0, |targets| targets.len());
        let entries = PRIORITIES
            .iter()
            .map(|(priority, name)| {
                let (id, priority) = (id.clone(), *priority);
                Entry::new(*name, move |root, _, cx| {
                    if !root.session.bulk(Bulk::Priority(priority)) {
                        root.session.set_priority(&id, priority);
                        root.session.review_refined();
                    }
                    root.repaint_after(crate::session::PRIORITY_DURATION, cx);
                })
                .mark(Mark::Priority(priority))
                .key(priority_key(priority))
                .checked(marked == 0 && current == Some(priority))
            })
            .collect();
        self.open_menu(
            entries,
            "Set priority",
            anchor,
            MenuOptions {
                choice: Some(choice_title("Set priority", marked)),
                ..MenuOptions::default()
            },
            window,
            cx,
        );
    }

    pub(crate) fn change_status(
        &mut self,
        _: &ChangeStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_status_at(self.anchor("status"), window, cx);
    }

    pub(crate) fn change_priority(
        &mut self,
        _: &ChangePriority,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_priority_at(self.anchor("priority"), window, cx);
    }

    pub(crate) fn change_project(
        &mut self,
        _: &ChangeProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_row() else {
            return;
        };
        let marked = self.marked_rows();
        let linked = self
            .session
            .detail
            .as_ref()
            .is_some_and(|detail| detail.epic.is_some());
        let epic = marked.iter().any(|row| row.is_epic);
        if epic || (marked.is_empty() && (row.is_epic || linked)) {
            self.session.show_toast(
                Toast::info("Epic children stay in the epic's project")
                    .detail("Remove them from the epic first"),
            );
            cx.notify();
            return;
        }
        let entries = self
            .session
            .snapshot
            .projects
            .iter()
            .map(|project| {
                let (id, key) = (row.id.clone(), project.key.clone());
                Entry::new(project.name.clone(), move |root, _, _| {
                    if !root.session.bulk(Bulk::Project(key.clone())) {
                        root.session
                            .write(Command::SetProject(id.clone(), key.clone()));
                    }
                })
                .hint(project.prefix.clone())
                .mark(Mark::Project(project.key.clone()))
            })
            .collect();
        let anchor = self.anchor("project");
        self.open_menu(
            entries,
            "Move to project",
            anchor,
            MenuOptions::default(),
            window,
            cx,
        );
    }

    pub(crate) fn edit_labels(
        &mut self,
        _: &EditLabels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_row() else {
            return;
        };
        let id = row.id.clone();
        let marked = self.marked_rows();
        let rows = if marked.is_empty() {
            vec![row.clone()]
        } else {
            marked.clone()
        };
        let on_all = |label: &String| rows.iter().all(|row| row.labels.contains(label));
        let on_some = |label: &String| rows.iter().any(|row| row.labels.contains(label));
        // A label only some tasks carry stays as it is unless it gets checked.
        let some: Vec<String> = (rows.iter().flat_map(|row| row.labels.iter()))
            .filter(|label| !on_all(label))
            .cloned()
            .collect();
        let save: Rc<dyn Fn(&mut Root)> = Rc::new(move |root| {
            let labels = root.menu_checked();
            let partial = (some.iter())
                .filter(|label| !labels.contains(label))
                .cloned()
                .collect();
            let bulk = Bulk::Labels {
                selected: labels.clone(),
                partial,
            };
            if !root.session.bulk(bulk) {
                root.session.write(Command::SetLabels(id.clone(), labels));
            }
        });
        let entry = move |label: String, save: Rc<dyn Fn(&mut Root)>| {
            Entry::new(label, move |root, _, _| save(root))
        };
        let mut labels = self.session.snapshot.labels.clone();
        labels.extend(rows.iter().flat_map(|row| row.labels.iter().cloned()));
        labels.sort();
        labels.dedup();
        let entries = labels
            .into_iter()
            .map(|label| {
                let hint = if on_some(&label) && !on_all(&label) {
                    "Some"
                } else {
                    ""
                };
                let checked = on_all(&label);
                entry(label, save.clone()).checked(checked).hint(hint)
            })
            .collect();
        let create = Rc::new(move |label: String| entry(label, save.clone()));
        let anchor = self.anchor("labels");
        let options = MenuOptions {
            multi: true,
            create: Some(create),
            ..MenuOptions::default()
        };
        self.open_menu(entries, "Toggle labels", anchor, options, window, cx);
    }

    fn pick_links(&mut self, removing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), Some(detail)) = (self.target_id(), self.session.detail.clone()) else {
            return;
        };
        let linked = |task: &TaskId| detail.blocked_by.iter().any(|link| link.id == *task);
        let cycle = |task: &TaskId| detail.blocks.iter().any(|link| link.id == *task);
        let entries = self
            .session
            .catalog
            .iter()
            .filter(|task| task.id != id)
            .filter(|task| {
                if removing {
                    linked(&task.id)
                } else {
                    task.status.is_open() || linked(&task.id)
                }
            })
            .map(|task| {
                let (from, to) = (id.clone(), task.id.clone());
                let title = task.title.clone();
                Entry::new(task.title.clone(), move |root, _, _| {
                    let command = if root.menu_checked().contains(&title) {
                        Command::AddBlocker(from.clone(), to.clone())
                    } else {
                        Command::RemoveBlocker(from.clone(), to.clone())
                    };
                    root.session.write(command);
                })
                .hint(if cycle(&task.id) {
                    "Would form a cycle".to_string()
                } else {
                    task.reference.clone()
                })
                .mark(Mark::Status(task.status))
                .checked(linked(&task.id))
                .off(cycle(&task.id))
            })
            .collect();
        let options = MenuOptions {
            multi: true,
            ..MenuOptions::default()
        };
        self.open_menu(
            entries,
            "Blocked by",
            self.anchor("add"),
            options,
            window,
            cx,
        );
    }

    pub(crate) fn add_blocker(
        &mut self,
        _: &AddBlocker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_links(false, window, cx);
    }

    pub(crate) fn remove_blocker(
        &mut self,
        _: &RemoveBlocker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_links(true, window, cx);
    }

    pub(crate) fn add_related(
        &mut self,
        _: &AddRelated,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(id), Some(detail)) = (self.target_id(), self.session.detail.clone()) else {
            return;
        };
        let entries = self
            .session
            .catalog
            .iter()
            .filter(|task| task.id != id)
            .map(|task| {
                let (from, to) = (id.clone(), task.id.clone());
                let title = task.title.clone();
                let linked = detail.related.iter().any(|link| link.id == task.id);
                Entry::new(task.title.clone(), move |root, _, _| {
                    let command = if root.menu_checked().contains(&title) {
                        Command::AddRelated(from.clone(), to.clone())
                    } else {
                        Command::RemoveRelated(from.clone(), to.clone())
                    };
                    root.session.write(command);
                })
                .hint(task.reference.clone())
                .mark(Mark::Status(task.status))
                .checked(linked)
            })
            .collect();
        let options = MenuOptions {
            multi: true,
            ..MenuOptions::default()
        };
        self.open_menu(
            entries,
            "Related to",
            self.anchor("add"),
            options,
            window,
            cx,
        );
    }

    pub(crate) fn edit_metadata(
        &mut self,
        _: &EditMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.target_id().is_none() {
            return;
        }
        let entries = self
            .session
            .snapshot
            .fields
            .iter()
            .map(|field| {
                let key = field.clone();
                Entry::new(field.clone(), move |root, window, cx| {
                    root.open_meta(key.clone(), window, cx);
                })
            })
            .collect();
        self.open_menu(
            entries,
            "Metadata field",
            self.anchor("add"),
            MenuOptions::default(),
            window,
            cx,
        );
    }

    pub(crate) fn add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.target_id().is_none() {
            return;
        }
        let entries = vec![
            Entry::action("Blocker", "", AddBlocker),
            Entry::action("Related task", "", AddRelated),
            Entry::new("Image", |root, _, cx| root.add_image_from_disk(cx)),
            Entry::action("Metadata field", "", EditMetadata),
            Entry::action("Note", "", FocusNote),
        ];
        self.open_menu(
            entries,
            "Add to task",
            self.anchor("add"),
            MenuOptions::default(),
            window,
            cx,
        );
    }

    pub(crate) fn overflow_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.target_row() else {
            return;
        };
        let last = if row.deleted {
            Entry::action("Restore", "t R", RestoreTask)
        } else {
            delete_entry()
        };
        let entries = vec![
            Entry::action("Expand", "↵", Expand).mark(Mark::Icon("icons/ic-expand.svg")),
            Entry::action("Copy ref", "y r", CopyRef).mark(Mark::Icon("icons/ic-copy.svg")),
            Entry::action("Copy task report", "y m", CopyReport)
                .mark(Mark::Icon("icons/ic-copy.svg")),
            Entry::action("Mark canceled", "x", CancelTask)
                .mark(Mark::Status(TaskStatus::Canceled)),
            last,
        ];
        let options = MenuOptions {
            plain: true,
            ..MenuOptions::default()
        };
        self.open_menu(
            entries,
            "Task actions",
            self.anchor("overflow"),
            options,
            window,
            cx,
        );
    }

    /// The menu a row opens under the pointer.
    pub(crate) fn row_menu(
        &mut self,
        id: TaskId,
        at: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Opens on mouse down, where the root would otherwise retake focus from the menu.
        window.prevent_default();
        self.session.open_row_menu(&id);
        self.select(id.clone(), window, cx);
        let Some(row) = self.session.snapshot.row(&id).cloned() else {
            return;
        };
        let anchor = Bounds::new(at, size(px(0.), px(0.)));
        let entries = vec![
            Entry::action("Expand", "↵", Expand).mark(Mark::Icon("icons/ic-expand.svg")),
            Entry::action("Done", "d", MarkDone)
                .mark(Mark::Status(TaskStatus::Done))
                .sep(),
            Entry::action("Cancel", "x", CancelTask).mark(Mark::Status(TaskStatus::Canceled)),
            Entry::new("Status", move |root, window, cx| {
                root.pick_status_at(anchor, window, cx);
            })
            .hint("s")
            .mark(Mark::Status(row.status))
            .sep(),
            Entry::new("Priority", move |root, window, cx| {
                root.pick_priority_at(anchor, window, cx);
            })
            .hint("!")
            .mark(Mark::Priority(row.priority)),
            Entry::action("Schedule", "e a", EditAvailability)
                .mark(Mark::Icon("icons/ic-upcoming.svg")),
            Entry::action("Labels", "e l", EditLabels).mark(Mark::Icon("icons/ic-labels.svg")),
            Entry::action("Copy ref", "y r", CopyRef)
                .mark(Mark::Icon("icons/ic-copy.svg"))
                .sep(),
            delete_entry(),
        ];
        let options = MenuOptions {
            plain: true,
            ..MenuOptions::default()
        };
        self.open_menu(entries, "Task actions", anchor, options, window, cx);
    }

    pub(crate) fn pick_filter(
        &mut self,
        _: &PickFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_filter_menu(None, window, cx);
    }

    pub(crate) fn pick_priority_filter(
        &mut self,
        _: &PickPriorityFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_filter_menu(Some(FilterKind::Priority), window, cx);
    }

    pub(crate) fn pick_label_filter(
        &mut self,
        _: &PickLabelFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_filter_menu(Some(FilterKind::Label), window, cx);
    }

    pub(crate) fn pick_metadata_filter(
        &mut self,
        _: &PickMetadataFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_filter_menu(Some(FilterKind::Metadata), window, cx);
    }

    /// The filter menu, narrowed to one kind of filter when `only` names it.
    fn open_filter_menu(
        &mut self,
        only: Option<FilterKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snapshot = &self.session.snapshot;
        let view = snapshot.view.clone();
        let mut entries = Vec::new();
        let priorities = if only.is_none_or(|kind| kind == FilterKind::Priority) {
            PRIORITIES.as_slice()
        } else {
            &[]
        };
        if only.is_none() {
            entries.push(Entry::head("Priority"));
        }
        for &(priority, name) in priorities {
            entries.push(
                Entry::new(name, move |root, _, _| {
                    let on = root.menu_checked().iter().any(|label| label == name);
                    root.session
                        .write(Command::FilterPriority(on.then_some(priority)));
                })
                .mark(Mark::Priority(priority))
                .checked(view.priority == Some(priority))
                .group(1),
            );
        }
        if only.is_none() {
            entries.push(Entry::head("Labels"));
        }
        let labels = if only.is_none_or(|kind| kind == FilterKind::Label) {
            snapshot.labels.as_slice()
        } else {
            &[]
        };
        for label in labels {
            let value = label.clone();
            entries.push(
                Entry::new(label.clone(), move |root, _, _| {
                    let on = root.menu_checked().contains(&value);
                    let label = on.then(|| value.clone());
                    root.session.write(Command::FilterLabel(label));
                })
                .checked(view.label.as_ref() == Some(label))
                .group(2),
            );
        }
        if only.is_none() {
            entries.push(Entry::head("Metadata"));
        }
        let fields = if only.is_none_or(|kind| kind == FilterKind::Metadata) {
            snapshot.fields.as_slice()
        } else {
            &[]
        };
        for field in fields {
            for (present, word) in [(true, "present"), (false, "missing")] {
                let label = format!("{field} {word}");
                let (key, name) = (field.clone(), label.clone());
                let filter = MetaFilter {
                    key: key.clone(),
                    present,
                };
                entries.push(
                    Entry::new(label, move |root, _, _| {
                        let on = root.menu_checked().contains(&name);
                        let filter = on.then(|| MetaFilter {
                            key: key.clone(),
                            present,
                        });
                        root.session.write(Command::FilterMetadata(filter));
                    })
                    .checked(view.metadata.as_ref() == Some(&filter))
                    .group(3),
                );
            }
        }
        if only.is_none() {
            entries.push(Entry::head("Deleted"));
            for (filter, name) in [
                (DeletedFilter::Show, "Include deleted"),
                (DeletedFilter::Only, "Deleted only"),
            ] {
                entries.push(
                    Entry::new(name, move |root, _, _| {
                        let on = root.menu_checked().iter().any(|label| label == name);
                        let value = if on { filter } else { DeletedFilter::Hide };
                        root.session.write(Command::FilterDeleted(value));
                    })
                    .checked(view.deleted == filter)
                    .group(4),
                );
            }
        }
        let options = MenuOptions {
            multi: true,
            ..MenuOptions::default()
        };
        self.open_menu(
            entries,
            match only {
                None => "Filter this view",
                Some(FilterKind::Priority) => "Filter by priority",
                Some(FilterKind::Label) => "Filter by label",
                Some(FilterKind::Metadata) => "Filter by metadata",
            },
            self.anchor("filter"),
            options,
            window,
            cx,
        );
    }

    pub(crate) fn cycle_deleted(
        &mut self,
        _: &CycleDeleted,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = match self.session.snapshot.view.deleted {
            DeletedFilter::Hide => DeletedFilter::Show,
            DeletedFilter::Show => DeletedFilter::Only,
            DeletedFilter::Only => DeletedFilter::Hide,
        };
        self.session.write(Command::FilterDeleted(next));
        cx.notify();
    }

    pub(crate) fn clear_filters(
        &mut self,
        _: &ClearFilters,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session.write(Command::ClearFilters);
        cx.notify();
    }

    pub(crate) fn pick_order(
        &mut self,
        _: &PickOrder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = &self.session.snapshot.view;
        if matches!(view.query, TaskQuery::Upcoming | TaskQuery::Overdue) {
            let name = view_name(view.query);
            self.session
                .show_toast(Toast::info(format!("{name} has a fixed order")));
            cx.notify();
            return;
        }
        let queue = view.query == TaskQuery::Queue;
        let current = (!queue).then_some(view.order);
        let mut entries: Vec<Entry> = ORDERS
            .iter()
            .map(|(order, name, key)| {
                let order = *order;
                Entry::new(*name, move |root, _, _| {
                    root.session.write(Command::SetOrder(order));
                })
                .key(*key)
                .checked(current == Some(order))
            })
            .collect();
        entries.push(
            Entry::new("Reverse", |root, _, _| {
                root.session.write(Command::ReverseOrder);
            })
            .key('r')
            .sep(),
        );
        let title = if queue {
            "Order by · opens Open"
        } else {
            "Order by"
        };
        self.open_menu(
            entries,
            "Order by",
            self.anchor("order"),
            MenuOptions {
                choice: Some(title.to_string()),
                ..MenuOptions::default()
            },
            window,
            cx,
        );
    }

    pub(crate) fn pick_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.session.snapshot.view.group;
        let entries = GROUPS
            .iter()
            .map(|(group, name)| {
                let group = *group;
                Entry::new(*name, move |root, _, _| {
                    root.session.write(Command::SetGroup(group));
                })
                .hint(if current == group { "Current" } else { "" })
            })
            .collect();
        self.open_menu(
            entries,
            "Group by",
            self.anchor("group"),
            MenuOptions::default(),
            window,
            cx,
        );
    }

    pub(crate) fn delete_task(
        &mut self,
        _: &DeleteTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.target_id() {
            self.leave_task(window, cx);
            if !self.session.bulk(Bulk::Deleted(true)) {
                self.session.delete(&id);
            }
            cx.notify();
        }
    }

    pub(crate) fn restore_task(&mut self, _: &RestoreTask, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_rows().iter().any(|row| row.deleted) {
            self.session.bulk(Bulk::Deleted(false));
            cx.notify();
            return;
        }
        match self.target_row().filter(|row| row.deleted) {
            Some(row) => self.session.write(Command::SetDeleted(row.id, false)),
            None => self
                .session
                .show_toast(Toast::info("Select a deleted task to restore")),
        }
        cx.notify();
    }

    fn move_lane(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(row) = self.target_row() else {
            return;
        };
        let at = LANES.iter().position(|status| *status == row.status);
        let Some(at) = at else {
            return;
        };
        let next = (at as isize + delta).clamp(0, LANES.len() as isize - 1) as usize;
        if LANES[next] != row.status {
            self.session.set_status(&row.id, LANES[next]);
            self.after_status_motion(cx);
        }
    }

    pub(crate) fn lane_back(&mut self, _: &LaneBack, _: &mut Window, cx: &mut Context<Self>) {
        self.move_lane(-1, cx);
    }

    pub(crate) fn lane_on(&mut self, _: &LaneOn, _: &mut Window, cx: &mut Context<Self>) {
        self.move_lane(1, cx);
    }

    pub(crate) fn undo(&mut self, _: &input::Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.session.undo();
        cx.notify();
    }

    pub(crate) fn focus_note(
        &mut self,
        _: &FocusNote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.target().is_some() {
            self.details_open = true;
            let focus = self.note.read(cx).focus.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    fn copy_text(&mut self, text: String, what: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.session.copied_toast(what);
        cx.notify();
    }

    pub(crate) fn copy_id(&mut self, _: &CopyId, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.target_id() {
            self.copy_text(id.to_string(), "ID", cx);
        }
    }

    pub(crate) fn copy_title(&mut self, _: &CopyTitle, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.target_row() {
            self.copy_text(row.title, "title", cx);
        }
    }

    pub(crate) fn copy_description(
        &mut self,
        _: &CopyDescription,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(detail) = self.session.detail.clone() {
            self.copy_text(detail.description, "description", cx);
        }
    }

    pub(crate) fn copy_title_and_description(
        &mut self,
        _: &CopyTitleAndDescription,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(detail) = self.session.detail.clone() {
            let text = format!("{}\n\n{}", detail.row.title, detail.description);
            self.copy_text(text, "title and description", cx);
        }
    }

    pub(crate) fn copy_notes(&mut self, _: &CopyNotes, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(detail) = self.session.detail.clone() {
            let notes: Vec<&str> = detail.notes.iter().map(|note| note.body.as_str()).collect();
            self.copy_text(notes.join("\n\n"), "notes", cx);
        }
    }

    pub(crate) fn copy_report(&mut self, _: &CopyReport, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.target_id() {
            self.session.write(Command::CopyReport(id));
            cx.notify();
        }
    }

    pub(crate) fn open_palette(
        &mut self,
        _: &OpenPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snapshot = &self.session.snapshot;
        let mut entries = Vec::new();
        if let Some(toast) = self.session.toast()
            && let Some(action) = &toast.action
        {
            entries.extend([
                Entry::head("Toast"),
                Entry::action(
                    format!("{}: {}", action.label(), toast.title),
                    "⌘↩",
                    RunToastAction,
                ),
            ]);
        }
        entries.extend([
            Entry::head("Task"),
            Entry::action("New task", "a", NewTask),
            Entry::action("Search tasks", "/", OpenSearch),
        ]);
        if let Some(row) = self.session.target_row() {
            entries.extend([
                Entry::action("Mark done", "d", MarkDone),
                Entry::action("Mark canceled", "x", CancelTask),
                Entry::action("Choose status", "s", ChangeStatus),
                Entry::action("Choose priority", "!", ChangePriority),
                Entry::action("Edit schedule", "e a", EditAvailability),
                Entry::action("Edit labels", "e l", EditLabels),
                Entry::action("Move to project", "e j", ChangeProject),
                Entry::action("Add blocker", "t B", AddBlocker),
                Entry::action("Add related task", "t l", AddRelated),
                Entry::action("Edit metadata", "e m", EditMetadata),
                Entry::action("Add note", "n", FocusNote),
                Entry::action("Rename task", "e t", RenameTask),
                Entry::action("Expand task", "↵", Expand),
                Entry::action("Add to epic", "t c", AddToEpic),
                Entry::action("Publish report as secret gist", "t g", PublishGist),
            ]);
            entries.push(if row.deleted {
                Entry::action("Restore task", "t R", RestoreTask)
            } else {
                Entry::action("Delete task", "t D", DeleteTask)
            });
            entries.extend([
                Entry::head("Copy"),
                Entry::action("Copy ref", "y r", CopyRef),
                Entry::action("Copy title", "y t", CopyTitle),
                Entry::action("Copy task report as Markdown", "y m", CopyReport),
            ]);
        }
        entries.extend([
            Entry::action("New epic", "", NewEpic),
            Entry::action("Review tasks", "", ReviewTasks),
            Entry::action("Mark all visible tasks", "t V", MarkVisible),
            Entry::action("Clear marks", "t C", ClearMarks),
            Entry::head("Go to"),
        ]);
        for (query, name, _, key, _) in VIEWS {
            entries.push(Entry::action(format!("Go to {name}"), key, ShowView(query)));
        }
        for project in &snapshot.projects {
            let key = project.key.clone();
            entries.push(
                Entry::new(
                    format!("Scope to {}", project.name),
                    move |root, window, cx| {
                        root.scope_to(Some(key.clone()), window, cx);
                    },
                )
                .hint("g p")
                .mark(Mark::Project(project.key.clone())),
            );
        }
        entries.push(Entry::action("All projects", "g a", AllProjects));
        entries.extend([
            Entry::head("App"),
            Entry::action("Keyboard shortcuts", "?", OpenHelp),
            Entry::action("About Aven", "", OpenAbout),
            Entry::action("Sync now", "S", SyncNow),
            Entry::action("Open sync", "", OpenSync),
            Entry::action("Settings", "⌘,", OpenSettings),
            Entry::action(
                if crate::theme::theme(cx).dark {
                    "Use light theme"
                } else {
                    "Use dark theme"
                },
                "",
                ToggleTheme,
            ),
        ]);
        let options = MenuOptions {
            wide: true,
            ..MenuOptions::default()
        };
        self.open_menu(
            entries,
            "Type a command",
            Bounds::default(),
            options,
            window,
            cx,
        );
    }

    /// Adds image files to the target, leaving anything else alone.
    pub(crate) fn attach_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let Some(id) = self.target_id() else {
            return;
        };
        for image in paths.iter().filter_map(|path| read_image(path)) {
            self.session.write(Command::AddImage(id.clone(), image));
        }
        cx.notify();
    }

    pub(crate) fn add_image_from_disk(&mut self, cx: &mut Context<Self>) {
        self.prompt_for_images(cx, Self::attach_paths);
    }

    pub(crate) fn prompt_for_images(
        &mut self,
        cx: &mut Context<Self>,
        then: fn(&mut Self, &[PathBuf], &mut Context<Self>),
    ) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add Image".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                this.update(cx, |this, cx| then(this, &paths, cx)).ok();
            }
        })
        .detach();
    }

    pub(crate) fn paste_image(&mut self, _: &PasteImage, _: &mut Window, cx: &mut Context<Self>) {
        let image = clipboard_image(cx).and_then(pasted_image);
        if let (Some(id), Some(image)) = (self.target_id(), image) {
            self.session.write(Command::AddImage(id, image));
            cx.notify();
        }
    }

    pub(crate) fn save_image_copy(
        &mut self,
        attachment: String,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let directory = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let receiver = cx.prompt_for_new_path(&directory, Some(name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = receiver.await {
                this.update(cx, |this, _| {
                    this.session.write(Command::SaveImage(attachment, path));
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn drop_on_lane(&mut self, id: &TaskId, status: TaskStatus, cx: &mut Context<Self>) {
        self.session.set_status(id, status);
        self.after_status_motion(cx);
    }
}

fn delete_entry() -> Entry {
    Entry::action("Delete", "t D", DeleteTask)
        .mark(Mark::Icon("icons/ic-trash.svg"))
        .sep()
        .danger()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::new(4, 4)
            .write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn a_pasted_tiff_becomes_a_png() {
        let image = storable("pasted.png".to_string(), encoded(image::ImageFormat::Tiff));
        assert_eq!(image.name, "pasted.png");
        assert_eq!(
            image::guess_format(&image.bytes).unwrap(),
            image::ImageFormat::Png
        );
    }

    #[test]
    fn a_tiff_file_keeps_its_name_with_a_png_extension() {
        let image = storable("scan.tiff".to_string(), encoded(image::ImageFormat::Tiff));
        assert_eq!(image.name, "scan.png");
    }

    #[test]
    fn stored_formats_and_undecodable_bytes_pass_through() {
        let png = encoded(image::ImageFormat::Png);
        assert_eq!(*storable("a.png".to_string(), png.clone()).bytes, png);
        let junk = b"not an image".to_vec();
        let image = storable("photo.heic".to_string(), junk.clone());
        assert_eq!((image.name.as_str(), &*image.bytes), ("photo.heic", &junk));
    }
}

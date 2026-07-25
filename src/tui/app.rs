use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use aven_core::db::Database;
use ratatui::widgets::{ListState, TableState};

use crate::config::AppConfig;
use crate::tui::app_intake::IntakeController;
use crate::tui::authoring::AuthoringState;
use crate::tui::bounded_history::BoundedHistory;
use crate::tui::conflict_flow::ConflictFlowState;
use crate::tui::overlay::OverlayState;
use crate::tui::shortcut_buffer::ShortcutBuffer;
use crate::tui::store::{TaskOrder, TaskViewState, TuiStore};
use crate::tui::toast::{Toast, ToastSeverity};

pub(crate) const TASK_ROW_DOUBLE_CLICK: Duration = Duration::from_millis(500);
pub(super) const MAX_EXTERNAL_IMAGE_EXPORTS: usize = 8;

#[cfg(not(test))]
fn default_image_viewer_launcher() -> fn(&std::path::Path) -> anyhow::Result<()> {
    crate::tui::platform::open_image_in_default_viewer
}

#[cfg(test)]
fn default_image_viewer_launcher() -> fn(&std::path::Path) -> anyhow::Result<()> {
    |_| Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRowClick {
    pub(crate) task_id: crate::ids::TaskId,
    pub(crate) viewport_row: u16,
    pub(crate) at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskRefKind {
    Short,
    Durable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskCopyKind {
    Title,
    Description,
    TitleAndDescription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Tasks,
}

pub(super) const SEARCH_PREVIEW_LIMIT: usize = 8;
const NAVIGATION_HISTORY_LIMIT: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notification {
    Toast {
        toast: Toast,
        created_at: Instant,
    },
    Loading {
        message: String,
        started_at: Instant,
    },
}

impl Notification {
    pub(crate) fn toast(message: impl Into<String>, severity: ToastSeverity) -> Self {
        Self::Toast {
            toast: Toast::new(message, severity),
            created_at: Instant::now(),
        }
    }

    pub(crate) fn loading(message: impl Into<String>) -> Self {
        Self::Loading {
            message: message.into(),
            started_at: Instant::now(),
        }
    }

    pub(crate) fn toast_view(&self) -> Toast {
        match self {
            Self::Toast { toast, .. } => toast.clone(),
            Self::Loading {
                message,
                started_at,
            } => Toast::new(
                format!("{} {message}", loading_frame(*started_at)),
                ToastSeverity::Info,
            )
            .without_icon(),
        }
    }
}

fn loading_frame(started_at: Instant) -> &'static str {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let elapsed = started_at.elapsed().as_millis() as usize;
    frames[(elapsed / 120) % frames.len()]
}

#[derive(Debug, Clone)]
pub(crate) struct WidgetState {
    pub(crate) sidebar: ListState,
    pub(crate) table: TableState,
    pub(crate) marked_task_ids: BTreeSet<crate::ids::TaskId>,
    pub(crate) inline_image_placements: Vec<crate::tui::ui::DetailInlineImagePlacement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FooterChoiceMode {
    Status,
    Priority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetailNavigationState {
    pub(super) task_id: crate::ids::TaskId,
    pub(super) scroll: u16,
    pub(super) focused_child_task_id: Option<crate::ids::TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LastChangeReturnState {
    pub(super) view_state: TaskViewState,
    pub(super) selected_task_id: Option<crate::ids::TaskId>,
    pub(super) selected_index: Option<usize>,
    pub(super) table_offset: usize,
}

pub(crate) struct App {
    pub(crate) store: TuiStore,
    pub(crate) should_quit: bool,
    pub(crate) focus: Focus,
    pub(super) intake: IntakeController,
    pub(crate) widgets: WidgetState,
    pub(crate) overlay: Option<OverlayState>,
    pub(super) edit_scope: crate::tui::app_edit::EditScope,
    pub(super) edit_aggregate: crate::tui::app_edit::EditAggregate,
    pub(super) onboarding_intro: Option<crate::tui::app_onboarding::OnboardingIntro>,
    pub(crate) notification: Option<Notification>,
    pub(super) pending_shortcut: ShortcutBuffer,
    pub(super) pending_shortcut_scroll: u16,
    pub(super) footer_choice_mode: Option<FooterChoiceMode>,
    pub(super) detail_context: bool,
    pub(super) sidebar_visible: bool,
    pub(super) detail_context_scroll: u16,
    pub(super) authoring: AuthoringState,
    pub(super) conflict_flow: ConflictFlowState,
    pub(super) pending_rename_project: Option<String>,
    pub(super) pending_delete_project: Option<String>,
    pub(super) pending_delete_attachment: Option<String>,
    pub(super) needs_terminal_clear: bool,
    pub(super) search: crate::tui::app_search::SearchController,
    pub(super) update: crate::tui::app_update::UpdateController,
    pub(super) next_refresh_at: Instant,
    pub(crate) last_task_click: Option<TaskRowClick>,
    pub(crate) hovered_detail_child_task_id: Option<crate::ids::TaskId>,
    pub(crate) selected_detail_child_task_id: Option<crate::ids::TaskId>,
    pub(crate) selected_detail_attachment_id: Option<String>,
    pub(crate) detail_text_selection: Option<crate::tui::detail_selection::DetailTextSelection>,
    pub(crate) detail_text_dragging: bool,
    pub(super) previous_inline_image_placements: Vec<crate::tui::ui::DetailInlineImagePlacement>,
    pub(super) previous_inline_image_backend: crate::tui::inline_images::InlineImageBackend,
    pub(super) deferred_inline_image_placements: Vec<crate::tui::ui::DetailInlineImagePlacement>,
    pub(super) inline_image_emission_at: Option<Instant>,
    pub(super) preview_controller: crate::tui::preview_controller::PreviewController,
    pub(super) attachment_controller: crate::tui::attachment_controller::AttachmentController,
    pub(super) image_viewer_launcher: fn(&std::path::Path) -> anyhow::Result<()>,
    pub(super) external_image_exports: Vec<(String, tempfile::TempDir)>,
    #[cfg(test)]
    pub(crate) inline_image_context_override: Option<crate::tui::ui::DetailInlineImageContext>,
    pub(super) navigation_history: BoundedHistory<TaskViewState>,
    pub(super) detail_navigation_history: BoundedHistory<DetailNavigationState>,
    pub(super) last_changed_task_id: Option<crate::ids::TaskId>,
    pub(super) last_change_return: Option<LastChangeReturnState>,
}

impl App {
    pub(crate) async fn new_with_view_state(
        database: Database,
        workspace: crate::workspaces::Workspace,
        view_state: TaskViewState,
    ) -> Result<Self> {
        Self::new_with_store(TuiStore::new_with_view_state(database, workspace, view_state).await?)
    }

    #[cfg(test)]
    pub(crate) async fn new_for_tests(database: Database) -> Result<Self> {
        let store = TuiStore::new(database, crate::workspaces::Workspace::default()).await?;
        Self::new_with_store(store)
    }

    fn new_with_store(store: TuiStore) -> Result<Self> {
        let next_refresh_at = store.last_refresh + crate::tui::app_lifecycle::REFRESH_INTERVAL;
        let mut app = Self {
            store,
            should_quit: false,
            focus: Focus::Tasks,
            intake: IntakeController::new(),
            widgets: WidgetState {
                sidebar: ListState::default(),
                table: TableState::default(),
                marked_task_ids: BTreeSet::new(),
                inline_image_placements: Vec::new(),
            },
            overlay: None,
            edit_scope: crate::tui::app_edit::EditScope::None,
            edit_aggregate: crate::tui::app_edit::EditAggregate::Uniform,
            onboarding_intro: None,
            notification: None,
            pending_shortcut: ShortcutBuffer::default(),
            pending_shortcut_scroll: 0,
            footer_choice_mode: None,
            detail_context: false,
            sidebar_visible: true,
            detail_context_scroll: 0,
            authoring: AuthoringState::default(),
            conflict_flow: ConflictFlowState::default(),
            pending_rename_project: None,
            pending_delete_project: None,
            pending_delete_attachment: None,
            needs_terminal_clear: false,
            search: crate::tui::app_search::SearchController::new(),
            update: crate::tui::app_update::UpdateController::new(),
            next_refresh_at,
            last_task_click: None,
            hovered_detail_child_task_id: None,
            selected_detail_child_task_id: None,
            selected_detail_attachment_id: None,
            detail_text_selection: None,
            detail_text_dragging: false,
            previous_inline_image_placements: Vec::new(),
            previous_inline_image_backend: crate::tui::inline_images::InlineImageBackend::None,
            deferred_inline_image_placements: Vec::new(),
            inline_image_emission_at: None,
            preview_controller: crate::tui::preview_controller::PreviewController::new(),
            attachment_controller: crate::tui::attachment_controller::AttachmentController::new(),
            image_viewer_launcher: default_image_viewer_launcher(),
            external_image_exports: Vec::new(),
            #[cfg(test)]
            inline_image_context_override: None,
            navigation_history: BoundedHistory::new(NAVIGATION_HISTORY_LIMIT),
            detail_navigation_history: BoundedHistory::new(NAVIGATION_HISTORY_LIMIT),
            last_changed_task_id: None,
            last_change_return: None,
        };
        app.restore_sidebar_selection();
        app.widgets
            .table
            .select((app.store.main_row_count() > 0).then_some(0));
        Ok(app)
    }

    pub(crate) fn open_task_on_start(&mut self, task_id: &crate::ids::TaskId) -> Result<()> {
        if !self.select_task_by_id(task_id) {
            bail!("task target disappeared before the TUI loaded");
        }
        self.focus = Focus::Tasks;
        self.overlay = Some(OverlayState::Detail { scroll: 0 });
        Ok(())
    }

    pub(super) fn select_task_by_id(&mut self, task_id: &crate::ids::TaskId) -> bool {
        let Some(index) = self
            .store
            .tasks
            .iter()
            .position(|item| &item.task.id == task_id)
        else {
            return false;
        };
        self.widgets.table.select(Some(index));
        true
    }

    pub(crate) fn set_config(&mut self, config: AppConfig) {
        self.store.task_columns = config.tui.columns.clone();
        self.intake.set_config(config);
    }

    pub(crate) fn set_add_task_db_path(&mut self, db_path: std::path::PathBuf) {
        self.intake.set_db_path(db_path);
    }

    pub(crate) fn begin_command(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Command {
            state: crate::tui::overlay::CommandState::blank(),
        });
    }

    pub(super) fn push_navigation_state(&mut self, previous: TaskViewState) {
        if previous == self.store.view_state {
            return;
        }
        self.navigation_history.push(previous);
    }

    pub(super) fn clear_navigation_history(&mut self) {
        self.navigation_history.clear();
    }

    pub(super) fn push_detail_navigation_state(&mut self, previous: DetailNavigationState) {
        self.detail_navigation_history.push(previous);
    }

    pub(super) fn detail_has_parent(&self) -> bool {
        !self.detail_navigation_history.is_empty()
    }

    pub(super) fn clear_detail_session(&mut self) {
        self.pending_shortcut.clear();
        self.pending_shortcut_scroll = 0;
        self.detail_navigation_history.clear();
        self.selected_detail_child_task_id = None;
        self.selected_detail_attachment_id = None;
        self.hovered_detail_child_task_id = None;
        self.detail_text_selection = None;
        self.detail_text_dragging = false;
        self.last_task_click = None;
        self.detail_context = false;
        self.detail_context_scroll = 0;
        self.overlay = None;
    }

    pub(super) fn go_back_in_detail(&mut self) -> bool {
        let mut skipped = false;
        while let Some(previous) = self.detail_navigation_history.pop() {
            let Some(index) = self
                .store
                .tasks
                .iter()
                .position(|item| item.task.id == previous.task_id)
            else {
                skipped = true;
                continue;
            };
            self.widgets.table.select(Some(index));
            self.focus = Focus::Tasks;
            self.detail_context = false;
            self.detail_context_scroll = previous.scroll;
            self.selected_detail_attachment_id = None;
            self.selected_detail_child_task_id = previous.focused_child_task_id.filter(|task_id| {
                self.store.tasks[index]
                    .epic_children
                    .iter()
                    .any(|child| &child.task_id == task_id)
            });
            self.overlay = Some(crate::tui::overlay::OverlayState::Detail {
                scroll: previous.scroll,
            });
            if skipped {
                self.set_warning("some previous tasks are hidden by the current view");
            }
            return true;
        }
        if skipped {
            self.set_warning("previous tasks are hidden by the current view");
        }
        false
    }

    pub(super) async fn go_back(&mut self) -> Result<()> {
        let Some(previous) = self.navigation_history.pop() else {
            self.set_info("no previous navigation state");
            return Ok(());
        };
        let result = self.store.restore_view_state(previous).await?;
        self.apply_filter_selection(result.selected);
        if let Some(project) = result.fallback_scope {
            self.set_warning(format!("project scope {project} is no longer available"));
        } else {
            self.set_info("returned to previous navigation state");
        }
        Ok(())
    }

    pub(super) fn clear_live_search_preview(&mut self) {
        self.search.cancel();
    }

    pub(super) fn search_preview_work_pending(&self) -> bool {
        self.search.work_pending()
    }

    pub(super) async fn set_sort(&mut self, sort: TaskOrder) -> Result<()> {
        let previous = self.store.view_state.clone();
        let selected = self.store.set_order(sort).await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        self.set_info(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    pub(super) async fn reverse_sort(&mut self) -> Result<()> {
        let previous = self.store.view_state.clone();
        let selected = self.store.reverse_sort().await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        self.set_info(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    pub(super) fn set_info(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Info);
    }

    pub(super) fn set_warning(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Warning);
    }

    pub(super) fn set_error(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Error);
    }

    pub(super) fn set_success(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Success);
    }

    fn set_toast(&mut self, message: impl Into<String>, severity: ToastSeverity) {
        self.notification = Some(Notification::toast(message, severity));
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

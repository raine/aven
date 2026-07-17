use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;
use tokio::task::JoinHandle;

use crate::config::AppConfig;
use crate::operations::TaskDraft;
use crate::tui::authoring::AuthoringState;
use crate::tui::conflict_flow::ConflictFlowState;
use crate::tui::overlay::OverlayState;
use crate::tui::shortcut_buffer::ShortcutBuffer;
use crate::tui::store::{TaskOrder, TaskViewState, TuiStore};
use crate::tui::toast::{Toast, ToastSeverity};

pub(crate) const TASK_ROW_DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRowClick {
    pub(crate) task_id: String,
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
pub(super) enum NaturalRetry {
    AddTask,
    Dialog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Tasks,
}

pub(super) struct PendingTaskIntake {
    pub(super) handle: JoinHandle<Result<TaskDraft>>,
    pub(super) retry: NaturalRetry,
    pub(super) value: String,
    pub(super) create_on_success: bool,
}

pub(super) struct ReadyTaskIntake {
    pub(super) outcome: Result<TaskDraft>,
    pub(super) retry: NaturalRetry,
    pub(super) value: String,
    pub(super) create_on_success: bool,
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
    pub(crate) marked_task_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FooterChoiceMode {
    Status,
    Priority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetailNavigationState {
    pub(super) task_id: String,
    pub(super) scroll: u16,
}

pub(crate) struct App {
    pub(crate) store: TuiStore,
    pub(crate) should_quit: bool,
    pub(crate) focus: Focus,
    pub(super) add_task_db_path: Option<PathBuf>,
    pub(crate) widgets: WidgetState,
    pub(crate) overlay: Option<OverlayState>,
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
    pub(super) needs_terminal_clear: bool,
    pub(super) add_task_only: bool,
    pub(super) add_task_only_message: Option<String>,
    pub(super) add_task_config: AppConfig,
    pub(super) pending_task_intake: Option<PendingTaskIntake>,
    pub(super) ready_task_intake: Option<ReadyTaskIntake>,
    pub(super) search: crate::tui::app_search::SearchController,
    pub(super) update: crate::tui::app_update::UpdateController,
    pub(super) next_refresh_at: Instant,
    pub(crate) last_task_click: Option<TaskRowClick>,
    pub(crate) hovered_detail_child_task_id: Option<String>,
    pub(crate) selected_detail_child_task_id: Option<String>,
    pub(crate) detail_text_selection: Option<crate::tui::detail_selection::DetailTextSelection>,
    pub(crate) detail_text_dragging: bool,
    pub(super) navigation_history: Vec<TaskViewState>,
    pub(super) detail_navigation_history: Vec<DetailNavigationState>,
}

impl App {
    pub(crate) async fn new(pool: SqlitePool, project: Option<&str>) -> Result<Self> {
        let store = match project {
            Some("") => TuiStore::new_for_inferred_project(pool).await?,
            Some(project) => TuiStore::new_for_project(pool, project).await?,
            None => TuiStore::new(pool).await?,
        };
        Self::new_with_store(store)
    }

    #[cfg(test)]
    pub(crate) async fn new_for_tests(pool: SqlitePool) -> Result<Self> {
        let store = TuiStore::new(pool).await?;
        Self::new_with_store(store)
    }

    fn new_with_store(store: TuiStore) -> Result<Self> {
        let next_refresh_at = store.last_refresh + crate::tui::app_lifecycle::REFRESH_INTERVAL;
        let mut app = Self {
            store,
            should_quit: false,
            focus: Focus::Tasks,
            widgets: WidgetState {
                sidebar: ListState::default(),
                table: TableState::default(),
                marked_task_ids: BTreeSet::new(),
            },
            overlay: None,
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
            needs_terminal_clear: false,
            add_task_only: false,
            add_task_only_message: None,
            add_task_db_path: None,
            add_task_config: AppConfig::default(),
            pending_task_intake: None,
            ready_task_intake: None,
            search: crate::tui::app_search::SearchController::new(),
            update: crate::tui::app_update::UpdateController::new(),
            next_refresh_at,
            last_task_click: None,
            hovered_detail_child_task_id: None,
            selected_detail_child_task_id: None,
            detail_text_selection: None,
            detail_text_dragging: false,
            navigation_history: Vec::new(),
            detail_navigation_history: Vec::new(),
        };
        app.restore_sidebar_selection();
        app.widgets
            .table
            .select(Some(0).filter(|_| !app.store.tasks.is_empty()));
        Ok(app)
    }

    pub(crate) fn set_config(&mut self, config: AppConfig) {
        self.store.task_columns = config.tui.columns.clone();
        self.add_task_config = config;
    }

    pub(crate) fn set_add_task_db_path(&mut self, db_path: PathBuf) {
        self.add_task_db_path = Some(db_path);
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
        if self.navigation_history.last() == Some(&previous) {
            return;
        }
        if self.navigation_history.len() >= NAVIGATION_HISTORY_LIMIT {
            self.navigation_history.remove(0);
        }
        self.navigation_history.push(previous);
    }

    pub(super) fn clear_navigation_history(&mut self) {
        self.navigation_history.clear();
    }

    pub(super) fn push_detail_navigation_state(&mut self, previous: DetailNavigationState) {
        if self.detail_navigation_history.last() == Some(&previous) {
            return;
        }
        if self.detail_navigation_history.len() >= NAVIGATION_HISTORY_LIMIT {
            self.detail_navigation_history.remove(0);
        }
        self.detail_navigation_history.push(previous);
    }

    pub(super) fn go_back_in_detail(&mut self) -> bool {
        let Some(previous) = self.detail_navigation_history.pop() else {
            return false;
        };
        let Some(index) = self
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == previous.task_id)
        else {
            self.set_warning("previous task is hidden by the current view");
            return false;
        };
        self.widgets.table.select(Some(index));
        self.focus = Focus::Tasks;
        self.detail_context = false;
        self.detail_context_scroll = previous.scroll;
        self.overlay = Some(crate::tui::overlay::OverlayState::Detail {
            scroll: previous.scroll,
        });
        true
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

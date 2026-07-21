use std::time::Instant;

use anyhow::{Result, bail};
use aven_core::db::Database;

use crate::config::AppConfig;
use crate::tui::app_intake::IntakeController;
use crate::tui::authoring::AuthoringState;
use crate::tui::inline_image_surface::InlineImageSurface;
use crate::tui::list_surface::ListSurface;
pub(crate) use crate::tui::list_surface::{Focus, LastChangeReturnState};
use crate::tui::overlay::OverlayState;
use crate::tui::shortcut_buffer::ShortcutBuffer;
use crate::tui::store::{TaskOrder, TaskViewState, TuiStore};
use crate::tui::toast::{Toast, ToastSeverity};

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

pub(super) const SEARCH_PREVIEW_LIMIT: usize = 8;

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
    pub(crate) inline_image_placements: Vec<crate::tui::ui::DetailInlineImagePlacement>,
    pub(crate) detail_document: Option<std::rc::Rc<crate::tui::ui::DetailDocument>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FooterChoiceMode {
    Status,
    Priority,
}

#[derive(Debug, Clone)]
pub(super) struct FooterChoiceState {
    pub(super) mode: FooterChoiceMode,
    pub(super) selection: crate::tui::task_selection::TaskSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DetailSection {
    EpicParent,
    EpicChildren,
    Attachments,
    DependsOn,
    Blocks,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DetailTargetId {
    Task {
        section: DetailSection,
        task_id: crate::ids::TaskId,
    },
    Attachment {
        attachment_id: String,
    },
    Expand {
        section: DetailSection,
    },
}

impl DetailTargetId {
    pub(crate) fn section(&self) -> DetailSection {
        match self {
            Self::Task { section, .. } | Self::Expand { section } => *section,
            Self::Attachment { .. } => DetailSection::Attachments,
        }
    }

    pub(crate) fn attachment_id(&self) -> Option<&str> {
        match self {
            Self::Attachment { attachment_id } => Some(attachment_id),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn task_id(&self) -> Option<&crate::ids::TaskId> {
        match self {
            Self::Task { task_id, .. } => Some(task_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RemovedEpicChild {
    pub(crate) epic_id: crate::ids::TaskId,
    pub(crate) child: crate::query::TaskDependencyLink,
    pub(crate) original_position: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeriesRowClick {
    pub(crate) series_id: aven_core::recurrence::RecurrenceSeriesId,
    pub(crate) viewport_row: u16,
    pub(crate) at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SeriesDetailReturn {
    pub(super) series_id: aven_core::recurrence::RecurrenceSeriesId,
    pub(super) scroll: u16,
}

pub(crate) struct App {
    pub(crate) store: TuiStore,
    pub(crate) should_quit: bool,
    pub(crate) list: ListSurface,
    pub(super) intake: IntakeController,
    pub(crate) widgets: WidgetState,
    pub(crate) overlay: Option<OverlayState>,
    pub(super) onboarding_intro: Option<crate::tui::app_onboarding::OnboardingIntro>,
    pub(crate) notification: Option<Notification>,
    pub(super) pending_shortcut: ShortcutBuffer,
    pub(super) pending_shortcut_scroll: u16,
    pub(super) footer_choice: Option<FooterChoiceState>,
    pub(crate) detail: crate::tui::detail_session::DetailSession,
    pub(super) authoring: AuthoringState,
    pub(super) needs_terminal_clear: bool,
    pub(super) search: crate::tui::app_search::SearchController,
    pub(super) update: crate::tui::app_update::UpdateController,
    pub(super) next_refresh_at: Instant,
    pub(crate) last_series_click: Option<SeriesRowClick>,
    pub(super) series_detail_return: Option<SeriesDetailReturn>,
    pub(super) inline_images: InlineImageSurface,
    pub(super) preview_controller: crate::tui::preview_controller::PreviewController,
    pub(super) attachment_controller: crate::tui::attachment_controller::AttachmentController,
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
        let has_tasks = store.main_row_count() > 0;
        let mut app = Self {
            store,
            should_quit: false,
            list: ListSurface::new(has_tasks),
            intake: IntakeController::new(),
            widgets: WidgetState {
                inline_image_placements: Vec::new(),
                detail_document: None,
            },
            overlay: None,
            onboarding_intro: None,
            notification: None,
            pending_shortcut: ShortcutBuffer::default(),
            pending_shortcut_scroll: 0,
            footer_choice: None,
            detail: crate::tui::detail_session::DetailSession::inactive(),
            authoring: AuthoringState::default(),
            needs_terminal_clear: false,
            search: crate::tui::app_search::SearchController::new(),
            update: crate::tui::app_update::UpdateController::new(),
            next_refresh_at,
            last_series_click: None,
            series_detail_return: None,
            inline_images: InlineImageSurface::new(),
            preview_controller: crate::tui::preview_controller::PreviewController::new(),
            attachment_controller: crate::tui::attachment_controller::AttachmentController::new(),
        };
        app.restore_sidebar_selection();
        Ok(app)
    }

    pub(crate) fn open_task_on_start(&mut self, task_id: &crate::ids::TaskId) -> Result<()> {
        if !self.select_task_by_id(task_id) {
            bail!("task target disappeared before the TUI loaded");
        }
        self.list.focus_tasks();
        self.show_detail(0);
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
        self.list.select_task(Some(index));
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
        self.list.push_navigation(previous, &self.store.view_state);
    }

    pub(super) fn clear_navigation_history(&mut self) {
        self.list.clear_navigation();
    }

    #[cfg(test)]
    pub(super) fn push_detail_navigation_state(
        &mut self,
        previous: crate::tui::detail_session::DetailSnapshot,
    ) {
        if self.detail.is_inactive() {
            self.detail = crate::tui::detail_session::DetailSession::open(0);
        }
        if let Some(detail) = self.detail.state_mut() {
            detail.push_history(previous);
        }
    }

    pub(super) fn detail_has_parent(&self) -> bool {
        self.detail
            .state()
            .is_some_and(|detail| detail.has_parent())
    }

    pub(super) fn show_detail(&mut self, scroll: u16) {
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(scroll);
        } else {
            self.detail = crate::tui::detail_session::DetailSession::open(scroll);
        }
        self.overlay = None;
    }

    pub(super) fn clear_detail_session(&mut self) {
        self.pending_shortcut.clear();
        self.pending_shortcut_scroll = 0;
        self.detail.close();
        self.overlay = None;
    }

    pub(super) async fn go_back_in_detail(&mut self) -> Result<bool> {
        let mut skipped = false;
        while let Some(previous) = self
            .detail
            .state_mut()
            .and_then(|detail| detail.pop_history())
        {
            let Some(item) = self.store.load_task_item(&previous.task_id).await? else {
                skipped = true;
                continue;
            };
            self.store.view_state = previous.view_state.clone();
            let selected = self.store.refresh(Some(&previous.task_id)).await?;
            let index = if let Some(index) = selected.filter(|&index| {
                self.store
                    .tasks
                    .get(index)
                    .is_some_and(|item| item.task.id == previous.task_id)
            }) {
                index
            } else {
                self.store.show_exact_task(item);
                0
            };
            self.list.select_task(Some(index));
            self.list.focus_tasks();
            if let Some(detail) = self.detail.state_mut() {
                detail.restore_snapshot(&previous);
            }
            self.show_detail(previous.scroll);
            if skipped {
                self.set_warning("some previous linked tasks are unavailable");
            }
            return Ok(true);
        }
        if skipped {
            self.set_warning("previous linked tasks are unavailable");
        }
        Ok(false)
    }

    pub(super) async fn go_back(&mut self) -> Result<()> {
        let Some(previous) = self.list.pop_navigation() else {
            self.set_info("no previous navigation state");
            return Ok(());
        };
        let return_anchor = self.series_detail_return.take();
        let selected = return_anchor.as_ref().map(|anchor| {
            crate::tui::store::MainRowSelection::RecurrenceSeries(anchor.series_id.clone())
        });
        let result = self
            .store
            .restore_view_state(previous, selected.as_ref())
            .await?;
        self.apply_filter_selection(result.selected);
        let mut navigation_warning = false;
        if let Some(anchor) = return_anchor
            && self.store.view_state.view == crate::tui::store::TaskView::Recurring
        {
            if self
                .store
                .selected_recurrence_series(result.selected)
                .is_some_and(|item| item.series.id == anchor.series_id)
            {
                self.store
                    .load_recurrence_series_detail(&anchor.series_id)
                    .await?;
                self.detail = crate::tui::detail_session::DetailSession::open(anchor.scroll);
                self.overlay = None;
            } else {
                self.store.recurrence_detail = None;
                self.detail.close();
                self.overlay = None;
                self.set_warning("recurring series is hidden by the restored filters");
                navigation_warning = true;
            }
        }
        if let Some(project) = result.fallback_scope {
            self.set_warning(format!("project scope {project} is no longer available"));
        } else if !navigation_warning {
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

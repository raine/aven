use std::time::Instant;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;

use crate::config::AppConfig;
use crate::tui::app_intake::IntakeController;
use crate::tui::authoring::AuthoringState;
use crate::tui::detail_session::{DetailSnapshot, DetailState};
use crate::tui::event::SINGLE_TASK_COPY_ACTIONS;
use crate::tui::gist_controller::GistController;
use crate::tui::inline_image_surface::InlineImageSurface;
use crate::tui::inline_images::{InlineImageBackend, active_backend_from_env};
pub(crate) use crate::tui::list_surface::{Focus, LastChangeReturnState, RecentActionReturnState};
use crate::tui::list_surface::{ListSurface, NavigationState};
use crate::tui::overlay::OverlayState;
use crate::tui::shortcut_buffer::ShortcutBuffer;
use crate::tui::store::{
    MainRowAnchor, MainRowIdentity, MainRowPosition, SelectionRestore, TaskOrder, TaskQuery,
    TaskViewState, TuiStore,
};
use crate::tui::sync_controller::SyncController;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailHistoryDirection {
    Back,
    Forward,
}

impl DetailHistoryDirection {
    fn pop(self, detail: &mut DetailState) -> Option<DetailSnapshot> {
        match self {
            Self::Back => detail.pop_history(),
            Self::Forward => detail.pop_forward_history(),
        }
    }

    fn push_current(self, detail: &mut DetailState, snapshot: DetailSnapshot) {
        match self {
            Self::Back => detail.push_forward_history(snapshot),
            Self::Forward => detail.push_back_history(snapshot),
        }
    }

    fn unavailable_message(self, found_target: bool) -> &'static str {
        match (self, found_target) {
            (Self::Back, true) => "some previous linked tasks are unavailable",
            (Self::Back, false) => "previous linked tasks are unavailable",
            (Self::Forward, true) => "some next linked tasks are unavailable",
            (Self::Forward, false) => "next linked tasks are unavailable",
        }
    }
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

#[derive(Debug, Clone, Default)]
pub(crate) struct WidgetState {
    pub(crate) inline_image_placements: Vec<crate::tui::ui::DetailInlineImagePlacement>,
    pub(crate) detail_document: Option<std::rc::Rc<crate::tui::ui::DetailDocument>>,
    /// Cell holding the caret of the focused text input in the last rendered
    /// frame, parked on the terminal cursor after every draw so IME preedit
    /// text appears where the user is typing.
    pub(crate) text_cursor: Option<ratatui::layout::Position>,
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
    Notes,
    DependsOn,
    Blocks,
    Related,
    Activity,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DetailTargetId {
    Task {
        section: DetailSection,
        task_id: crate::ids::TaskId,
    },
    Note {
        note_id: String,
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
            Self::Note { .. } => DetailSection::Notes,
            Self::Attachment { .. } => DetailSection::Attachments,
        }
    }

    pub(crate) fn routing_domain(&self) -> crate::tui::event::RoutingDomain {
        match self {
            Self::Task { .. } => crate::tui::event::RoutingDomain::DetailRelated,
            Self::Note { .. } | Self::Expand { .. } => {
                crate::tui::event::RoutingDomain::DetailPassive
            }
            Self::Attachment { .. } => crate::tui::event::RoutingDomain::DetailAttachment,
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
    pub(super) sync: SyncController,
    pub(super) gist: GistController,
    pub(super) changelog: crate::tui::changelog::ChangelogController,
    pub(super) next_refresh_at: Instant,
    pub(crate) last_series_click: Option<SeriesRowClick>,
    pub(super) series_detail_return: Option<SeriesDetailReturn>,
    pub(super) inline_images: InlineImageSurface,
    pub(super) inline_image_backend: InlineImageBackend,
    pub(super) preview_controller: crate::tui::preview_controller::PreviewController,
    pub(super) attachment_controller: crate::tui::attachment_controller::AttachmentController,
    pub(crate) command_catalog: std::sync::Arc<crate::tui::event::CommandCatalog>,
    pub(super) custom_command_planning: crate::tui::custom_command::CustomCommandPlanningContext,
    pub(super) custom_commands: crate::tui::custom_command_runtime::CustomCommandController,
    pub(super) pending_terminal_command:
        Option<crate::tui::custom_command::CustomCommandInvocation>,
    pub(super) terminal_mouse_capture: bool,
    #[cfg(test)]
    pub(super) _test_database_dir: Option<tempfile::TempDir>,
}

impl App {
    #[cfg(test)]
    pub(crate) async fn new_with_view_state(
        database: Database,
        workspace: crate::workspaces::Workspace,
        view_state: TaskViewState,
    ) -> Result<Self> {
        Self::new_with_store(TuiStore::new_with_view_state(database, workspace, view_state).await?)
    }

    pub(crate) async fn new_with_view_state_and_config(
        database: Database,
        workspace: crate::workspaces::Workspace,
        view_state: TaskViewState,
        config: AppConfig,
    ) -> Result<Self> {
        Self::new_with_store(
            TuiStore::new_with_view_state_and_config(database, workspace, view_state, config)
                .await?,
        )
    }

    #[cfg(test)]
    pub(crate) async fn new_for_tests(database: Database) -> Result<Self> {
        let store = TuiStore::new(database, crate::workspaces::Workspace::default()).await?;
        Self::new_with_store(store)
    }

    fn new_with_store(store: TuiStore) -> Result<Self> {
        let config = store.config().clone();
        let origin_cwd =
            std::env::current_dir().context("could not determine current directory")?;
        let blob_dir = crate::config::resolve_blob_dir(store.database_path(), &config).ok();
        let custom_command_planning = crate::tui::custom_command::CustomCommandPlanningContext {
            origin_cwd,
            home_dir: dirs::home_dir(),
            tui_pid: std::process::id(),
            aven_exe: std::env::current_exe().ok(),
            config_dir: crate::config::config_dir_path().ok(),
            db_path: store
                .database_file_identity()
                .map(std::path::Path::to_path_buf),
            blob_dir,
        };
        let next_refresh_at = store.last_refresh + crate::tui::app_lifecycle::REFRESH_INTERVAL;
        let has_tasks = store.main_row_count() > 0;
        let mut app = Self {
            store,
            should_quit: false,
            list: ListSurface::new(has_tasks),
            intake: IntakeController::new(),
            widgets: WidgetState::default(),
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
            update: crate::tui::app_update::UpdateController::new(config.update.automatic_checks),
            sync: SyncController::new(),
            gist: GistController::new(),
            changelog: crate::tui::changelog::ChangelogController::new(),
            next_refresh_at,
            last_series_click: None,
            series_detail_return: None,
            inline_images: InlineImageSurface::new(),
            inline_image_backend: active_backend_from_env(config.local.inline_images),
            preview_controller: crate::tui::preview_controller::PreviewController::new(),
            attachment_controller: crate::tui::attachment_controller::AttachmentController::new(),
            command_catalog: std::sync::Arc::new(crate::tui::event::CommandCatalog::default()),
            custom_command_planning,
            custom_commands: crate::tui::custom_command_runtime::CustomCommandController::default(),
            pending_terminal_command: None,
            terminal_mouse_capture: false,
            #[cfg(test)]
            _test_database_dir: None,
        };
        app.set_config(config);
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
        self.command_catalog = std::sync::Arc::new(crate::tui::event::CommandCatalog::new(
            config.tui.commands.clone(),
        ));
        self.custom_command_planning.blob_dir =
            crate::config::resolve_blob_dir(self.store.database_path(), &config).ok();
        self.store.set_config(config.clone());
        self.store.task_columns = config.tui.columns.clone();
        self.inline_image_backend = active_backend_from_env(config.local.inline_images);
        self.update
            .set_automatic_checks(config.update.automatic_checks);
        self.intake.set_config(config);
    }

    pub(crate) fn set_add_task_db_path(&mut self, db_path: std::path::PathBuf) {
        self.intake.set_db_path(db_path);
    }

    fn highlighted_sidebar_command_target(
        &self,
    ) -> Option<crate::tui::event::SidebarCommandTarget> {
        use crate::tui::event::SidebarCommandTarget;
        use crate::tui::store::SidebarEntryTarget;

        self.list
            .selected_sidebar()
            .and_then(|index| self.store.sidebar_entries.get(index))
            .and_then(|entry| entry.target.as_ref())
            .map(|target| match target {
                SidebarEntryTarget::View(view) => SidebarCommandTarget::View(*view),
                SidebarEntryTarget::Scope(scope) => SidebarCommandTarget::from_scope(scope),
            })
    }

    pub(super) fn current_routing_domain(&self) -> crate::tui::event::RoutingDomain {
        use crate::tui::event::RoutingDomain;
        if self.detail.is_active() {
            return self
                .detail
                .state()
                .and_then(|detail| detail.focused_target())
                .map_or(RoutingDomain::DetailParent, DetailTargetId::routing_domain);
        }
        if self.intake.view().add_task_only {
            return RoutingDomain::Normal;
        }
        let focused_sidebar = (self.list.focus() == crate::tui::list_surface::Focus::Sidebar)
            .then(|| self.highlighted_sidebar_command_target())
            .flatten();
        let is_empty = self.store.main_row_count() == 0;
        if self.store.view_state.query == crate::tui::store::TaskQuery::Recurring {
            return crate::tui::event::recurrence_list_situation(
                self.selected_recurrence_target_id().is_some(),
                &focused_sidebar,
                is_empty,
                None,
            )
            .routing_domain();
        }
        crate::tui::event::list_situation(
            self.bulk_scope_marked_task_count(),
            self.store
                .selected_task(self.list.selected_task())
                .is_some(),
            &focused_sidebar,
            is_empty,
            None,
        )
        .routing_domain()
    }

    pub(super) fn capture_command_session(
        &self,
        recurrence_series_id: Option<aven_core::recurrence::RecurrenceSeriesId>,
    ) -> crate::tui::event::CommandSessionSnapshot {
        use crate::tui::event::{
            CommandSessionSnapshot, CommandSurfaceSnapshot, CommandWorkspaceSnapshot,
            DetailCommandFocus,
        };
        use crate::tui::store::TaskQuery;

        let workspace = CommandWorkspaceSnapshot {
            id: self.store.active_workspace.id.clone(),
            key: self.store.active_workspace.key.clone(),
            name: self.store.active_workspace.name.clone(),
        };
        if self.intake.view().add_task_only {
            return CommandSessionSnapshot {
                workspace,
                surface: CommandSurfaceSnapshot::AddTaskOnly,
                recurrence_series_id,
            };
        }
        let focused_sidebar = (self.list.focus() == crate::tui::list_surface::Focus::Sidebar)
            .then(|| self.highlighted_sidebar_command_target())
            .flatten();
        let selected = self.store.selected_task(self.list.selected_task());
        if self.detail.is_active()
            && let Some(parent) = selected
        {
            let focus = self
                .detail
                .state()
                .and_then(|detail| detail.focused_target())
                .filter(|target| {
                    crate::tui::ui::detail_target_is_actionable(parent, target)
                        || matches!(
                            target,
                            DetailTargetId::Task {
                                section: DetailSection::EpicChildren,
                                task_id,
                            } if self.detail.state().and_then(|detail| detail.removed_epic_child()).is_some_and(
                                |removed| removed.epic_id == parent.task.id
                                    && removed.child.task_id == *task_id
                            )
                        )
                })
                .map(|target| match target {
                    DetailTargetId::Task {
                        section: DetailSection::EpicChildren,
                        task_id,
                    } => DetailCommandFocus::Relationship {
                        section: DetailSection::EpicChildren,
                        task_id: task_id.clone(),
                    },
                    DetailTargetId::Task { section, task_id } => DetailCommandFocus::Relationship {
                        section: *section,
                        task_id: task_id.clone(),
                    },
                    DetailTargetId::Note { .. } => DetailCommandFocus::Note,
                    DetailTargetId::Attachment { attachment_id } => {
                        let bytes_present = parent.attachments.iter().any(|attachment| {
                            attachment.attachment_id == *attachment_id
                                && self.attachment_bytes_are_available(attachment)
                        });
                        DetailCommandFocus::Attachment {
                            attachment_id: attachment_id.clone(),
                            bytes_present,
                        }
                    }
                    DetailTargetId::Expand { .. } => DetailCommandFocus::Disclosure
                });
            return CommandSessionSnapshot {
                workspace,
                surface: CommandSurfaceSnapshot::Detail {
                    parent_task_id: parent.task.id.clone(),
                    marked_task_ids: self.marked_task_ids_in_view(),
                    focus,
                    scroll: self.detail.state().map_or(0, |detail| detail.scroll()),
                },
                recurrence_series_id,
            };
        }
        let is_empty = self.store.main_row_count() == 0;
        let empty_preferred_action = is_empty
            .then(|| {
                if self.store.view_state.is_columns() {
                    crate::tui::ui::column_board_empty_state(&self.store)
                } else {
                    match self.store.view_state.query {
                        TaskQuery::Recurring => crate::tui::ui::recurrence_empty_state(&self.store),
                        TaskQuery::RecentActions => {
                            crate::tui::ui::recent_actions_empty_state(&self.store)
                        }
                        _ => crate::tui::ui::task_empty_state(&self.store),
                    }
                }
            })
            .and_then(|state| state.action.map(|action| action.action));
        let surface = if self.store.view_state.query == TaskQuery::Recurring {
            CommandSurfaceSnapshot::RecurrenceList {
                focused_sidebar,
                is_empty,
                empty_preferred_action,
            }
        } else {
            let primary_task_id = selected.map(|item| item.task.id.clone());
            CommandSurfaceSnapshot::List {
                primary_task_id,
                marked_task_ids: self.marked_task_ids_in_view(),
                visible_task_ids: self
                    .store
                    .tasks
                    .iter()
                    .map(|item| item.task.id.clone())
                    .collect(),
                focused_sidebar,
                is_empty,
                empty_preferred_action,
            }
        };
        CommandSessionSnapshot {
            workspace,
            surface,
            recurrence_series_id,
        }
    }

    pub(crate) async fn begin_command(&mut self) {
        self.pending_shortcut.clear();
        let (target, mut unavailable) = match self.recurrence_command_context().await {
            Ok(context) => context,
            Err(error) => {
                let target = self.selected_recurrence_target_id().map(|target| {
                    crate::tui::overlay::OverlayTarget::RecurrenceSeries {
                        workspace_id: target.workspace_id,
                        series_id: target.series_id,
                    }
                });
                let unavailable = target
                    .as_ref()
                    .map(|_| {
                        crate::tui::app_recurrence::RECURRENCE_COMMAND_ACTIONS
                            .into_iter()
                            .map(|action| crate::tui::overlay::CommandAvailabilityOverride {
                                action,
                                reason: "recurrence availability could not be loaded",
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.set_error(format!("{error:#}"));
                (target, unavailable)
            }
        };
        if !self.detail.is_active() && !self.marked_task_ids_in_view().is_empty() {
            unavailable.extend(SINGLE_TASK_COPY_ACTIONS.into_iter().map(|action| {
                crate::tui::overlay::CommandAvailabilityOverride {
                    action,
                    reason: "requires one task",
                }
            }));
        }
        let recurrence_series_id = target.as_ref().map(|target| match target {
            crate::tui::overlay::OverlayTarget::RecurrenceSeries { series_id, .. } => {
                series_id.clone()
            }
        });
        let session = self.capture_command_session(recurrence_series_id);
        if session.marked_task_ids().len() > 1 {
            unavailable.extend(
                crate::tui::event::COMMANDS
                    .iter()
                    .filter(|command| {
                        matches!(
                            command.bulk_support(),
                            crate::tui::event::BulkSupport::SingleOnly(_)
                        )
                    })
                    .map(|command| crate::tui::overlay::CommandAvailabilityOverride {
                        action: command.action,
                        reason: "one task only",
                    }),
            );
        }
        let state = crate::tui::overlay::CommandState::new(
            session,
            self.command_catalog.clone(),
            unavailable,
        );
        self.overlay = Some(OverlayState::Command { state });
    }

    pub(super) fn main_row_anchor(&self) -> Option<MainRowAnchor> {
        let selected = self.list.selected_task()?;
        let identity = match self.store.view_state.query {
            TaskQuery::Recurring => MainRowIdentity::RecurrenceSeries(
                self.store
                    .selected_recurrence_series(Some(selected))?
                    .series
                    .id
                    .clone(),
            ),
            TaskQuery::RecentActions => MainRowIdentity::RecentAction(
                self.store
                    .selected_recent_action(Some(selected))?
                    .change_id
                    .clone(),
            ),
            _ => MainRowIdentity::Task(self.store.selected_task(Some(selected))?.task.id.clone()),
        };
        let position = if self.store.view_state.is_columns() {
            let (column, row) =
                crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                    .position(selected)?;
            MainRowPosition::Column { column, row }
        } else if self.store.view_state.query == TaskQuery::Epics {
            MainRowPosition::EpicVisualRow(crate::tui::ui::task_visual_row(&self.store, selected)?)
        } else {
            MainRowPosition::Flat(selected)
        };
        Some(MainRowAnchor { identity, position })
    }

    fn main_row_identity_at(&self, selected: usize) -> Option<MainRowIdentity> {
        match self.store.view_state.query {
            TaskQuery::Recurring => Some(MainRowIdentity::RecurrenceSeries(
                self.store
                    .selected_recurrence_series(Some(selected))?
                    .series
                    .id
                    .clone(),
            )),
            TaskQuery::RecentActions => Some(MainRowIdentity::RecentAction(
                self.store
                    .selected_recent_action(Some(selected))?
                    .change_id
                    .clone(),
            )),
            _ => Some(MainRowIdentity::Task(
                self.store.selected_task(Some(selected))?.task.id.clone(),
            )),
        }
    }

    pub(super) fn query_selection_restore(&self) -> SelectionRestore {
        self.main_row_anchor()
            .map(SelectionRestore::Anchor)
            .unwrap_or(SelectionRestore::Default)
    }

    pub(super) fn capture_navigation_state(&self) -> NavigationState {
        self.list
            .capture_navigation(self.store.view_state.clone(), self.main_row_anchor())
    }

    pub(super) fn push_navigation_state(&mut self, previous: NavigationState) {
        self.list.push_navigation(previous, &self.store.view_state);
    }

    pub(super) fn clear_navigation_history(&mut self) {
        self.list.clear_navigation();
        self.series_detail_return = None;
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
        self.navigate_detail_history(DetailHistoryDirection::Back)
            .await
    }

    pub(super) async fn go_forward_in_detail(&mut self) -> Result<bool> {
        self.navigate_detail_history(DetailHistoryDirection::Forward)
            .await
    }

    async fn navigate_detail_history(&mut self, direction: DetailHistoryDirection) -> Result<bool> {
        let current = self
            .store
            .selected_task(self.list.selected_task())
            .and_then(|item| {
                self.detail.state().map(|detail| {
                    detail.snapshot(item.task.id.clone(), self.store.view_state.clone())
                })
            });
        let mut skipped = false;
        while let Some(target) = self
            .detail
            .state_mut()
            .and_then(|detail| direction.pop(detail))
        {
            let Some(item) = self.store.load_task_item(&target.task_id).await? else {
                skipped = true;
                continue;
            };
            self.store.view_state = target.view_state.clone();
            let selected = self.store.refresh(Some(&target.task_id)).await?;
            let index = if let Some(index) = selected.filter(|&index| {
                self.store
                    .tasks
                    .get(index)
                    .is_some_and(|item| item.task.id == target.task_id)
            }) {
                index
            } else {
                self.store.show_exact_task(item);
                0
            };
            self.list.select_task(Some(index));
            self.list.focus_tasks();
            if let Some(detail) = self.detail.state_mut() {
                if let Some(current) = current.clone() {
                    direction.push_current(detail, current);
                }
                detail.restore_snapshot(&target);
                detail.select_sibling_task(&target.task_id);
            }
            self.show_detail(target.scroll);
            if skipped {
                self.set_warning(direction.unavailable_message(true));
            }
            return Ok(true);
        }
        if skipped {
            self.set_warning(direction.unavailable_message(false));
        }
        Ok(false)
    }

    fn apply_navigation_selection(&mut self, target: &NavigationState, selected: Option<usize>) {
        let identity_recovered = target.anchor.as_ref().is_some_and(|anchor| {
            selected
                .and_then(|index| self.main_row_identity_at(index))
                .is_some_and(|identity| identity == anchor.identity)
        });
        self.apply_filter_selection(selected);
        if identity_recovered {
            self.list.set_task_offset(target.table_offset);
        }
    }

    pub(super) async fn go_back(&mut self) -> Result<()> {
        let current = self.capture_navigation_state();
        let Some(previous) = self.list.pop_navigation(current) else {
            self.set_info("no previous navigation state");
            return Ok(());
        };
        let returns_to_series = previous.view_state.query == TaskQuery::Recurring;
        let selected = returns_to_series
            .then_some(self.series_detail_return.as_ref())
            .flatten()
            .map(|anchor| {
                crate::tui::store::MainRowSelection::RecurrenceSeries(anchor.series_id.clone())
            });
        let restore = previous
            .anchor
            .clone()
            .map(SelectionRestore::Anchor)
            .or_else(|| selected.map(SelectionRestore::Identity))
            .unwrap_or(SelectionRestore::Default);
        let result = match self
            .store
            .restore_view_state_with_restore(previous.view_state.clone(), &restore)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let _ = self.list.pop_forward_navigation(previous.clone());
                return Err(error);
            }
        };
        self.apply_navigation_selection(&previous, result.selected);
        if returns_to_series && let Some(anchor) = self.series_detail_return.take() {
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
            }
        }
        if let Some(project) = result.fallback_scope {
            self.set_warning(format!("project scope {project} is no longer available"));
        }
        Ok(())
    }

    pub(super) async fn go_forward(&mut self) -> Result<()> {
        let current = self.capture_navigation_state();
        let Some(next) = self.list.pop_forward_navigation(current) else {
            self.set_info("no next navigation state");
            return Ok(());
        };
        let restore = next
            .anchor
            .clone()
            .map(SelectionRestore::Anchor)
            .unwrap_or_else(|| {
                next.selected_index
                    .map(SelectionRestore::Index)
                    .unwrap_or(SelectionRestore::Default)
            });
        let result = match self
            .store
            .restore_view_state_with_restore(next.view_state.clone(), &restore)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let _ = self.list.pop_navigation(next.clone());
                return Err(error);
            }
        };
        self.apply_navigation_selection(&next, result.selected);
        if let Some(project) = result.fallback_scope {
            self.set_warning(format!("project scope {project} is no longer available"));
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
        let previous = self.capture_navigation_state();
        let restore = previous
            .anchor
            .clone()
            .map(SelectionRestore::Anchor)
            .unwrap_or(SelectionRestore::Default);
        let selected = self.store.set_order_restoring(sort, &restore).await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    pub(super) async fn reverse_sort(&mut self) -> Result<()> {
        let previous = self.capture_navigation_state();
        let restore = previous
            .anchor
            .clone()
            .map(SelectionRestore::Anchor)
            .unwrap_or(SelectionRestore::Default);
        let selected = self.store.reverse_sort_restoring(&restore).await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
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

    pub(super) fn set_mutation_success(&mut self, message: impl Into<String>) {
        let mut message = message.into();
        if let Some(entry_id) = self.store.new_undo_entry_id.take()
            && self
                .store
                .available_undo()
                .is_some_and(|undo| undo.entry_id == entry_id)
        {
            const RETURN_ACTION: &str = " · g . return";
            if let Some(index) = message.find(RETURN_ACTION) {
                message.insert_str(index, " · u undo");
            } else {
                message.push_str(" · u undo");
            }
        }
        self.set_success(message);
    }

    fn set_toast(&mut self, message: impl Into<String>, severity: ToastSeverity) {
        self.notification = Some(Notification::toast(message, severity));
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

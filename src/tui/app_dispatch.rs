use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};

use std::time::{Duration, Instant};

use crate::tui::app::{App, DetailSection, DetailTargetId, Focus, FooterChoiceMode};
use crate::tui::authoring::AddTaskStep;
use crate::tui::detail_session::DetailTargetActivation;
use crate::tui::event::{
    Action, CommandCompletion, CommandSpecLookup, DetailFocusPolicy, command_cycle_options_for,
    complete_command_for, lookup_command_spec_for,
};
use crate::tui::input::key::{
    ImagePasteTarget, KeyInput, KeyRouteState, NormalKeyInput, route_key, route_normal_key,
};
use crate::tui::input::mouse::{
    MouseInput, PointerEvent, TaskSurfaceView, route_mouse, route_task_surface,
};
use crate::tui::navigation::{
    detail_scroll_with_delta_with_images, detail_task_delta, handle_detail_scroll_key_with_cap,
    handle_detail_scroll_key_with_images, next_index, scroll_with_delta,
};
use crate::tui::overlay::{
    AddTaskMode, CommandState, MultilineIntent, OverlayOutcome, OverlayState, PickerIntent,
    ScheduleEditorField, ScheduleEditorMode, TagComboboxIntent, UpdateOverlayState,
};
use crate::tui::platform::{copy_to_clipboard, is_editor_prefix_key, open_url_in_default_browser};
use crate::tui::shortcut_buffer::DetailShortcutResolution;
use crate::tui::store::TaskView;
use crate::tui::ui::{
    composer_help_scroll_cap, database_stats_scroll_cap, detail_copy_target_at,
    detail_help_scroll_cap, help_scroll_cap, prefix_hint_scroll_cap, task_at_position,
    task_status_at_position, text_panel_scroll_cap,
};

impl App {
    pub(super) async fn dispatch_paste(&mut self, text: &str) -> Result<()> {
        if self.paste_detail_image_from_text(text).await? {
            return Ok(());
        }
        if self.paste_add_task_image_from_text(text)? {
            return Ok(());
        }
        if text.is_empty() && self.paste_image_from_empty_terminal_paste().await? {
            return Ok(());
        }
        let Some(overlay) = self.overlay.take() else {
            return Ok(());
        };
        let mut overlay = crate::tui::overlay::handle_generic_overlay_paste(text, overlay);
        if let OverlayState::Search(state) = &mut overlay {
            self.handle_search_paste(state).await?;
        }
        self.overlay = Some(overlay);
        Ok(())
    }

    pub(crate) async fn dispatch_key(&mut self, key: KeyEvent, terminal_size: Size) -> Result<()> {
        let input = route_key(
            key,
            KeyRouteState {
                footer_choice: self.footer_choice.is_some(),
                shortcut_pending: !self.pending_shortcut.is_empty(),
                prefix_hints: self.prefix_hints_active(),
                overlay_captures: self.overlay_captures_input()
                    || (self.detail.is_active() && self.overlay.is_none()),
                detail_overlay: self.detail.is_active() && self.overlay.is_none(),
                detail_target_focused: self
                    .detail
                    .state()
                    .and_then(|detail| detail.focused_target())
                    .is_some(),
                add_task_image_target: matches!(
                    self.overlay,
                    Some(OverlayState::AddTask(_))
                        | Some(OverlayState::MultilineInput(
                            crate::tui::overlay::MultilineInputState {
                                intent: MultilineIntent::AddTaskNatural,
                                ..
                            }
                        ))
                ),
            },
            terminal_size.height,
        );
        match input {
            KeyInput::Action(action) => self.execute(action).await,
            KeyInput::PasteImage(ImagePasteTarget::Detail) => {
                self.paste_detail_image_from_clipboard().await
            }
            KeyInput::PasteImage(ImagePasteTarget::AddTask) => {
                self.paste_add_task_image_from_clipboard().await
            }
            KeyInput::FooterChoice(key) => self.handle_footer_choice_key(key).await,
            KeyInput::CancelShortcut => {
                self.pending_shortcut.cancel();
                self.pending_shortcut_scroll = 0;
                Ok(())
            }
            KeyInput::ToggleHelp => {
                self.toggle_help_at_height(terminal_size.height);
                Ok(())
            }
            KeyInput::ScrollPrefix(delta) => {
                self.dispatch_prefix_hint_scroll(delta, terminal_size);
                Ok(())
            }
            KeyInput::Overlay(key) => self.handle_overlay_key_at_size(key, terminal_size).await,
            KeyInput::Normal(code) => self.handle_normal_key(code).await,
            KeyInput::Ignore => Ok(()),
        }
    }

    async fn handle_footer_choice_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(choice) = self.footer_choice.clone() else {
            return Ok(());
        };
        if !key.modifiers.is_empty() {
            return Ok(());
        }
        match (choice.mode, key.code) {
            (_, KeyCode::Esc) => {
                self.footer_choice = None;
                Ok(())
            }
            (FooterChoiceMode::Status, KeyCode::Char('i')) => {
                self.submit_footer_status(choice.selection, "inbox").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('b')) => {
                self.submit_footer_status(choice.selection, "backlog").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('t')) => {
                self.submit_footer_status(choice.selection, "todo").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('a')) => {
                self.submit_footer_status(choice.selection, "active").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('d')) => {
                self.submit_footer_status(choice.selection, "done").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('x')) => {
                self.submit_footer_status(choice.selection, "canceled")
                    .await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('n')) => {
                self.submit_footer_priority(choice.selection, "none").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('l')) => {
                self.submit_footer_priority(choice.selection, "low").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('m')) => {
                self.submit_footer_priority(choice.selection, "medium")
                    .await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('h')) => {
                self.submit_footer_priority(choice.selection, "high").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('u')) => {
                self.submit_footer_priority(choice.selection, "urgent")
                    .await
            }
            _ => Ok(()),
        }
    }

    async fn submit_footer_status(
        &mut self,
        selection: crate::tui::task_selection::TaskSelection,
        status: &'static str,
    ) -> Result<()> {
        self.footer_choice = None;
        self.submit_edit_status(selection, status.to_string())
            .await?;
        Ok(())
    }

    async fn submit_footer_priority(
        &mut self,
        selection: crate::tui::task_selection::TaskSelection,
        priority: &'static str,
    ) -> Result<()> {
        self.footer_choice = None;
        self.submit_edit_priority(selection, false, priority.to_string())
            .await?;
        Ok(())
    }

    pub(crate) async fn dispatch_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<bool> {
        if mouse.kind != MouseEventKind::Moved {
            self.handle_mouse(mouse, terminal_size).await?;
            return Ok(true);
        }
        let previous_hover = self
            .detail
            .state()
            .and_then(|detail| detail.hovered_target())
            .cloned();
        self.handle_mouse(mouse, terminal_size).await?;
        Ok(self
            .detail
            .state()
            .and_then(|detail| detail.hovered_target())
            != previous_hover.as_ref())
    }

    async fn handle_mouse(&mut self, mouse: MouseEvent, terminal_size: Size) -> Result<()> {
        if matches!(self.overlay, Some(OverlayState::RecurrenceHistory(_))) {
            let Some(OverlayState::RecurrenceHistory(state)) = self.overlay.take() else {
                unreachable!("history overlay was matched")
            };
            return self
                .handle_recurrence_history_mouse(*state, mouse, terminal_size)
                .await;
        }
        match route_mouse(mouse.kind, self.prefix_hints_active()) {
            MouseInput::PrefixScroll(delta) => {
                self.dispatch_prefix_hint_scroll(delta, terminal_size);
                return Ok(());
            }
            MouseInput::OverlayScroll(kind) => {
                if self.dispatch_mouse_scroll(kind, terminal_size) {
                    return Ok(());
                }
                let delta = if kind == MouseEventKind::ScrollDown {
                    1
                } else {
                    -1
                };
                return self.handle_task_list_wheel(delta, terminal_size).await;
            }
            MouseInput::DetailPress => {
                if self
                    .handle_detail_attachment_mouse_click(mouse, terminal_size)
                    .await?
                {
                    return Ok(());
                }
                if self.begin_detail_text_selection(mouse, terminal_size) {
                    return Ok(());
                }
                if let Some(detail) = self.detail.state_mut() {
                    detail.clear_text_selection();
                }
            }
            MouseInput::DetailDrag => {
                self.update_detail_text_selection(mouse, terminal_size);
                return Ok(());
            }
            MouseInput::DetailRelease => {
                if let Some(detail) = self.detail.state_mut() {
                    detail.finish_text_drag();
                }
                return Ok(());
            }
            MouseInput::PointerMove => {
                self.handle_detail_mouse_move(mouse, terminal_size);
                return Ok(());
            }
            MouseInput::StatusPress => {
                if self.store.view_state.view == TaskView::Recurring {
                    if self.overlay.is_none()
                        && let Some(hit) = crate::tui::ui::recurrence_series_at_position(
                            &self.store,
                            self.list.table_state(),
                            self.task_area_for_mouse(terminal_size),
                            mouse.column,
                            mouse.row,
                        )
                    {
                        self.list.focus_tasks();
                        self.list.select_task(Some(hit.series_index));
                        self.last_series_click = None;
                        let target = crate::tui::app_recurrence::RecurrenceTargetId {
                            workspace_id: self.store.active_workspace.id.clone(),
                            series_id: hit.series_id,
                        };
                        self.begin_recurrence_context_menu(target).await?;
                    }
                    return Ok(());
                }
                return self
                    .handle_task_status_right_click(mouse, terminal_size)
                    .await;
            }
            MouseInput::Ignore => return Ok(()),
        }

        self.list.expire_task_click(Instant::now());

        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(OverlayState::Update(state)) = self.overlay.as_ref()
            && let Some(url) =
                crate::tui::ui::update_link_at(state, terminal_size, mouse.column, mouse.row)
        {
            if let Err(error) = open_url_in_default_browser(&url) {
                self.set_warning(format!("could not open release-note link: {error:#}"));
            }
            return Ok(());
        }

        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(OverlayState::Update(state)) = self.overlay.as_ref()
            && let Some(action) =
                crate::tui::ui::update_action_at(state, terminal_size, mouse.column, mouse.row)
        {
            let Some(OverlayState::Update(UpdateOverlayState::Available {
                plan,
                notes,
                scroll,
                cached,
                ..
            })) = self.overlay.take()
            else {
                return Ok(());
            };
            self.handle_update_overlay_key(
                UpdateOverlayState::Available {
                    plan,
                    notes,
                    scroll,
                    focus: action,
                    cached,
                },
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                terminal_size,
            )
            .await;
            return Ok(());
        }

        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(OverlayState::Changelog(state)) = self.overlay.as_ref()
            && let Some(url) = crate::tui::ui::changelog_link_at(
                &state.markdown,
                state.scroll,
                terminal_size,
                mouse.column,
                mouse.row,
            )
        {
            if let Err(error) = open_url_in_default_browser(&url) {
                self.set_warning(format!("could not open changelog link: {error:#}"));
            }
            return Ok(());
        }

        let header = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: terminal_size.width,
            height: 2,
        };
        if terminal_size.width >= 70
            && terminal_size.height >= 18
            && self.detail_underlay()
            && matches!(
                crate::tui::ui::header_target_at(
                    &self.store,
                    self.update.badge().as_ref(),
                    header,
                    mouse.column,
                    mouse.row,
                ),
                Some(crate::tui::ui::HeaderTarget::Home)
            )
        {
            self.clear_detail_session();
            return Ok(());
        }

        if matches!(self.overlay, Some(OverlayState::HeaderMenu(_))) {
            let Some(OverlayState::HeaderMenu(state)) = self.overlay.take() else {
                return Ok(());
            };
            self.submit_header_menu_at(state, mouse.column, mouse.row, terminal_size)
                .await?;
            return Ok(());
        }
        if matches!(self.overlay, Some(OverlayState::OrderMenu(_))) {
            let Some(OverlayState::OrderMenu(state)) = self.overlay.take() else {
                return Ok(());
            };
            self.submit_order_menu_at(state, mouse.column, mouse.row, terminal_size)
                .await?;
            return Ok(());
        }
        if self.detail.is_active() && self.overlay.is_none() {
            let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
            self.handle_detail_mouse_click(mouse, terminal_size, scroll)
                .await?;
            return Ok(());
        }
        let add_task_field = self.overlay.as_ref().and_then(|overlay| match overlay {
            OverlayState::AddTask(state) if state.mode == AddTaskMode::Compose => {
                crate::tui::ui::add_task_field_at(
                    Rect::new(0, 0, terminal_size.width, terminal_size.height),
                    self.intake.view().add_task_only,
                    crate::tui::ui::AddTaskLayout {
                        description: &state.description.lines,
                        mode: &state.mode,
                        has_attachments: !state.attachments.is_empty(),
                        show_schedule_error: state.schedule_error.is_some()
                            && state.schedule_validation_requested,
                    },
                    mouse.column,
                    mouse.row,
                )
            }
            _ => None,
        });
        if let Some(field) = add_task_field {
            let editable = matches!(
                self.overlay.as_ref(),
                Some(OverlayState::AddTask(state)) if state.is_step_editable(field)
            );
            if !editable {
                return Ok(());
            }
            if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                state.focus = field;
            }
            if field.is_metadata() {
                self.open_focused_add_task_control();
            }
            return Ok(());
        }
        if matches!(
            self.overlay,
            Some(OverlayState::Picker(_) | OverlayState::Confirm(_) | OverlayState::TextPanel(_))
        ) {
            self.list.clear_task_click();
            let Some(overlay) = self.overlay.take() else {
                return Ok(());
            };
            self.handle_overlay_mouse(overlay, mouse, terminal_size)
                .await?;
            return Ok(());
        }
        if self.overlay.is_some() || terminal_size.width < 70 || terminal_size.height < 18 {
            return Ok(());
        }
        if let Some(target) = crate::tui::ui::header_target_at(
            &self.store,
            self.update.badge().as_ref(),
            header,
            mouse.column,
            mouse.row,
        ) {
            self.list.clear_task_click();
            return match target {
                crate::tui::ui::HeaderTarget::Home => Ok(()),
                crate::tui::ui::HeaderTarget::Changelog => {
                    self.show_changelog();
                    Ok(())
                }
                crate::tui::ui::HeaderTarget::Workspace { column } => {
                    self.show_workspace_menu(column, mouse.row).await?;
                    Ok(())
                }
                crate::tui::ui::HeaderTarget::Scope { column } => {
                    self.show_scope_menu(column, mouse.row);
                    Ok(())
                }
                crate::tui::ui::HeaderTarget::View { column } => {
                    self.show_view_menu(column, mouse.row);
                    Ok(())
                }
                crate::tui::ui::HeaderTarget::MetricView(view) => self.show_view(view).await,
                crate::tui::ui::HeaderTarget::Order { column } => {
                    self.show_order_menu(column, mouse.row);
                    Ok(())
                }
                crate::tui::ui::HeaderTarget::Update => {
                    self.begin_update();
                    Ok(())
                }
                crate::tui::ui::HeaderTarget::SyncStatus => self.show_config_status(),
            };
        }

        let outside_sidebar = !self.sidebar_contains_mouse(terminal_size, mouse.column, mouse.row);
        let pointer = route_task_surface(
            TaskSurfaceView {
                store: &self.store,
                list: &self.list,
                terminal_area: Rect::new(0, 0, terminal_size.width, terminal_size.height),
                task_area: self.task_area_for_mouse(terminal_size),
                outside_sidebar,
            },
            mouse.column,
            mouse.row,
        );

        match pointer {
            PointerEvent::MoveToColumn(status) => {
                self.list.clear_task_click();
                let Some(selection) = self.resolve_task_selection() else {
                    self.set_info("no selected task to move");
                    return Ok(());
                };
                self.move_tasks_to_column(selection, status.as_str().to_string())
                    .await?;
            }
            PointerEvent::SelectRecentAction(action_index) => {
                self.list.focus_tasks();
                self.list.select_task(Some(action_index));
                self.list.clear_task_click();
            }
            PointerEvent::SelectSeries(hit) => {
                self.list.focus_tasks();
                self.list.select_task(Some(hit.series_index));
                let now = Instant::now();
                let is_double_click = self.last_series_click.as_ref().is_some_and(|previous| {
                    previous.series_id == hit.series_id
                        && previous.viewport_row == hit.viewport_row
                        && now.duration_since(previous.at) <= Duration::from_millis(500)
                });
                if is_double_click {
                    self.last_series_click = None;
                    self.store
                        .load_recurrence_series_detail(&hit.series_id)
                        .await?;
                    self.detail = crate::tui::detail_session::DetailSession::open(0);
                } else {
                    self.last_series_click = Some(crate::tui::app::SeriesRowClick {
                        series_id: hit.series_id,
                        viewport_row: hit.viewport_row,
                        at: now,
                    });
                }
            }
            PointerEvent::EditStatus(hit) => {
                self.list.clear_task_click();
                self.list.focus_tasks();
                self.list.select_task(Some(hit.task_index));
                self.begin_status_picker();
            }
            PointerEvent::SelectTask(hit) => {
                self.list.focus_tasks();
                self.list.select_task(Some(hit.task_index));
                let is_double_click =
                    self.list
                        .register_task_click(hit.task_id, hit.viewport_row, Instant::now());
                if is_double_click {
                    self.show_detail(0);
                }
            }
            PointerEvent::SelectSidebar(entry_index) => {
                self.list.clear_task_click();
                self.list.select_sidebar(Some(entry_index));
                self.apply_sidebar_selection().await?;
            }
            PointerEvent::None => self.list.clear_task_click(),
        }

        Ok(())
    }

    fn task_area_for_mouse(&self, terminal_size: Size) -> Rect {
        let body_height = terminal_size.height.saturating_sub(4);
        let body = Rect::new(0, 2, terminal_size.width, body_height);
        if !self.list.sidebar_visible() || body.width < 100 {
            body
        } else {
            let sidebar_width = body.width.min(26);
            Rect::new(
                sidebar_width,
                body.y,
                body.width.saturating_sub(sidebar_width),
                body.height,
            )
        }
    }

    async fn handle_task_status_right_click(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
        self.list.clear_task_click();
        if self.overlay.is_some() || terminal_size.width < 70 || terminal_size.height < 18 {
            return Ok(());
        }
        if self.sidebar_contains_mouse(terminal_size, mouse.column, mouse.row) {
            return Ok(());
        }
        let hit = if self.store.view_state.view == TaskView::Columns {
            task_at_position(
                &self.store,
                self.list.table_state(),
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
        } else {
            task_status_at_position(
                &self.store,
                self.list.table_state(),
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
        };
        let Some(hit) = hit else {
            return Ok(());
        };

        self.list.focus_tasks();
        self.list.select_task(Some(hit.task_index));
        self.begin_status_picker();
        Ok(())
    }

    fn sidebar_contains_mouse(&self, terminal_size: Size, column: u16, row: u16) -> bool {
        let terminal = Rect::new(0, 0, terminal_size.width, terminal_size.height);
        crate::tui::ui::sidebar_layout_for(terminal, self.list.focus(), self.list.sidebar_visible())
            .is_some_and(|layout| {
                column >= layout.sidebar.x
                    && column < layout.sidebar.x.saturating_add(layout.sidebar.width)
                    && row >= layout.sidebar.y
                    && row < layout.sidebar.y.saturating_add(layout.sidebar.height)
            })
    }

    pub(crate) async fn open_attachment_externally(&mut self, attachment_id: &str) {
        let Some(db_path) = self.intake.db_path() else {
            self.set_error("could not resolve attachment storage");
            return;
        };
        let Ok(blob_dir) = crate::config::resolve_blob_dir(db_path, self.intake.config()) else {
            self.set_error("could not resolve attachment storage");
            return;
        };
        let mut export = match self
            .store
            .lease_image_export(&blob_dir, attachment_id)
            .await
        {
            Ok(export) => export,
            Err(error) => {
                let message = error.to_string();
                if message.contains("attachment-invalidated") {
                    self.set_warning("attachment is no longer available");
                } else if message.contains("attachment-blob-unavailable") {
                    self.set_warning("attachment bytes are unavailable");
                } else if message.contains("attachment-format-unsupported") {
                    self.set_warning("attachment format cannot be opened");
                } else {
                    self.set_error("could not prepare attachment for the image viewer");
                }
                return;
            }
        };
        let launch_result = self.inline_images.launch_external_viewer(export.path());
        let release_result = self.store.release_image_export(&mut export).await;
        if let Err(_error) = launch_result {
            self.set_error("could not start the default image viewer");
            return;
        }
        self.inline_images
            .retain_export(attachment_id.to_string(), export.into_directory());
        if release_result.is_err() {
            self.set_warning("image opened; attachment read protection expires automatically");
        } else {
            self.set_success("opened attachment in default image viewer");
        }
    }

    pub(super) async fn remove_selected_epic_child(&mut self) -> Result<()> {
        let detail = self.detail.is_active();
        let focused_child = match self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
        {
            Some(DetailTargetId::Task {
                section: DetailSection::EpicChildren,
                task_id,
            }) => Some(task_id),
            _ => None,
        };
        let focused_child = detail.then_some(focused_child).flatten();
        if let (Some(removed), Some(child_id)) = (
            self.detail
                .state()
                .and_then(|detail| detail.removed_epic_child()),
            focused_child,
        ) && removed.child.task_id == *child_id
        {
            self.set_info(format!(
                "{} is already removed from its epic",
                removed.child.display_ref
            ));
            return Ok(());
        }
        let Some(target) = self
            .store
            .resolve_epic_child_target(self.list.selected_task(), focused_child)
        else {
            if detail
                && self
                    .store
                    .selected_task(self.list.selected_task())
                    .is_some_and(|item| item.task.is_epic)
            {
                self.set_warning("Select a child with Tab first");
            } else {
                self.set_warning("Selected task does not belong to an epic");
            }
            return Ok(());
        };
        let mutation = self.store.remove_epic_child(target).await?;
        self.list.select_task(mutation.message.selected);
        if detail
            && mutation.changed
            && let Some(detail) = self.detail.state_mut()
        {
            detail.set_removed_epic_child(Some(crate::tui::app::RemovedEpicChild {
                epic_id: mutation.epic.epic_id,
                child: mutation.child.clone(),
                original_position: mutation.original_position,
            }));
            detail.set_focused_target(Some(DetailTargetId::Task {
                section: DetailSection::EpicChildren,
                task_id: mutation.child.task_id,
            }));
        }
        self.set_success(mutation.message.message);
        Ok(())
    }

    fn open_detail_attachment(&mut self, attachment_id: String, scroll: u16) {
        self.list.clear_task_click();
        if let Some(detail) = self.detail.state_mut() {
            detail.clear_text_selection();
            detail.set_focused_target(Some(DetailTargetId::Attachment {
                attachment_id: attachment_id.clone(),
            }));
        }
        self.overlay = Some(OverlayState::AttachmentPreview {
            attachment_id,
            scroll,
        });
    }

    async fn handle_detail_attachment_mouse_click(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<bool> {
        if self.detail.is_inactive() || self.overlay.is_some() {
            return Ok(false);
        }
        let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
        let Some(context) = self.inline_image_context() else {
            return Ok(false);
        };
        let hit = self
            .detail_document_for_query(terminal_size)
            .and_then(|document| document.target_at_position(mouse.column, mouse.row));
        let Some(hit) = hit else {
            return Ok(false);
        };
        match hit {
            DetailTargetId::Task { task_id, .. } => {
                self.open_detail_task(&task_id, scroll).await;
            }
            target => {
                let activation = self
                    .detail
                    .state_mut()
                    .map(|detail| detail.activate_target(target));
                match activation {
                    Some(DetailTargetActivation::ToggleSection(section)) => {
                        self.activate_detail_disclosure(section, terminal_size);
                        let scroll = self.detail_focus_scroll(scroll, terminal_size);
                        if let Some(detail) = self.detail.state_mut() {
                            detail.set_scroll(scroll);
                        }
                        self.show_detail(scroll);
                    }
                    Some(DetailTargetActivation::OpenAttachment(attachment_id)) => {
                        let has_inline_placement = self
                            .widgets
                            .inline_image_placements
                            .iter()
                            .any(|placement| placement.attachment_id == attachment_id);
                        if context.previews_enabled && has_inline_placement {
                            self.open_detail_attachment(attachment_id, scroll);
                        } else {
                            self.open_attachment_externally(&attachment_id).await;
                        }
                    }
                    Some(DetailTargetActivation::Focus) => {
                        self.show_detail(scroll);
                    }
                    Some(DetailTargetActivation::FollowTask(_)) | None => {}
                }
            }
        }
        Ok(true)
    }

    async fn handle_detail_mouse_click(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
        scroll: u16,
    ) -> Result<bool> {
        if let Some(item) = self.store.selected_task(self.list.selected_task())
            && let Some(hit) = detail_copy_target_at(
                item,
                terminal_size.width,
                terminal_size.height,
                mouse.column,
                mouse.row,
            )
        {
            match copy_to_clipboard(&hit.value) {
                Ok(()) => self.set_success(format!("copied {}", hit.value)),
                Err(error) => self.set_error(format!("copy failed: {error}")),
            }
            return Ok(true);
        }

        let Some((target, _column, _row)) = crate::tui::ui::detail_metadata_target_at(
            terminal_size.width,
            terminal_size.height,
            mouse.column,
            mouse.row,
        ) else {
            return Ok(false);
        };
        self.list.clear_task_click();
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(scroll);
        }
        match target {
            crate::tui::ui::DetailMetadataTarget::Status => self.begin_status_picker(),
            crate::tui::ui::DetailMetadataTarget::Priority => self.begin_edit_priority(),
        }
        if self.overlay.is_none() {
            self.show_detail(scroll);
        }
        Ok(true)
    }

    fn handle_detail_mouse_move(&mut self, mouse: MouseEvent, terminal_size: Size) {
        if self.detail.is_inactive() || self.overlay.is_some() {
            if let Some(detail) = self.detail.state_mut() {
                detail.set_hovered_target(None);
            }
            return;
        }
        let hovered = self
            .detail_document_for_query(terminal_size)
            .and_then(|document| document.target_at_position(mouse.column, mouse.row));
        if let Some(detail) = self.detail.state_mut() {
            detail.set_hovered_target(hovered);
        }
    }

    async fn handle_task_list_wheel(&mut self, delta: isize, terminal_size: Size) -> Result<()> {
        if self.overlay.is_some()
            || terminal_size.width < 70
            || terminal_size.height < 18
            || self.detail_underlay()
            || self.list.focus() != Focus::Tasks
        {
            return Ok(());
        }

        let next = if self.store.view_state.view == TaskView::Columns {
            crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                .move_vertical_bounded(self.list.selected_task(), delta)
        } else {
            next_index(
                self.list.selected_task(),
                self.store.main_row_count(),
                delta,
                false,
            )
        };
        self.list.select_task(next);
        Ok(())
    }

    fn dispatch_prefix_hint_scroll(&mut self, delta: isize, terminal_size: Size) -> bool {
        if !self.prefix_hints_active() {
            return false;
        }
        let cap = prefix_hint_scroll_cap(
            terminal_size.height,
            self.detail_underlay(),
            &self.pending_shortcut.labels(),
        );
        self.pending_shortcut_scroll = scroll_with_delta(self.pending_shortcut_scroll, delta, cap);
        true
    }

    fn prefix_hints_active(&self) -> bool {
        if self.pending_shortcut.is_empty() {
            return false;
        }
        !matches!(
            &self.overlay,
            Some(OverlayState::AddTask(_))
                if self.pending_shortcut.has_add_task_status_prefix()
                    || self.pending_shortcut.has_add_task_priority_prefix()
        )
    }

    fn dispatch_mouse_scroll(&mut self, kind: MouseEventKind, terminal_size: Size) -> bool {
        let delta = match kind {
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::ScrollUp => -1,
            _ => return false,
        };

        let inline_images = self.inline_image_context();
        let detail_scroll_cap = self
            .detail_document_for_query(terminal_size)
            .map(|document| document.scroll_cap());
        if self.detail.is_active() && self.overlay.is_none() {
            let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
            let task = self.store.selected_task(self.list.selected_task());
            let scroll = if let Some(cap) = detail_scroll_cap {
                scroll_with_delta(scroll, delta, cap)
            } else {
                detail_scroll_with_delta_with_images(
                    scroll,
                    delta,
                    terminal_size.width,
                    terminal_size.height,
                    task,
                    inline_images.as_ref(),
                )
            };
            if let Some(detail) = self.detail.state_mut() {
                detail.set_scroll(scroll);
            }
            return true;
        }
        match &mut self.overlay {
            Some(OverlayState::Help { scroll }) => {
                let cap = help_scroll_cap(terminal_size.height);
                *scroll = scroll_with_delta(*scroll, delta, cap);
                true
            }
            Some(OverlayState::DetailHelp { scroll }) => {
                let cap = detail_help_scroll_cap(terminal_size.height);
                *scroll = scroll_with_delta(*scroll, delta, cap);
                true
            }
            Some(OverlayState::TextPanel(state)) => {
                let cap = text_panel_scroll_cap(&state.lines);
                state.scroll = scroll_with_delta(state.scroll, delta, cap);
                true
            }
            Some(OverlayState::Update(UpdateOverlayState::Available { notes, scroll, .. })) => {
                let cap = crate::tui::ui::update_notes_scroll_cap(notes, terminal_size);
                *scroll = scroll_with_delta(*scroll, delta, cap);
                true
            }
            Some(OverlayState::Changelog(state)) => {
                let cap =
                    crate::tui::changelog::changelog_scroll_cap(&state.markdown, terminal_size);
                state.scroll = scroll_with_delta(state.scroll, delta, cap);
                true
            }
            _ => false,
        }
    }

    fn overlay_captures_input(&self) -> bool {
        self.overlay
            .as_ref()
            .is_some_and(OverlayState::captures_input)
    }

    pub(crate) async fn handle_normal_key(&mut self, code: KeyCode) -> Result<()> {
        let translation =
            route_normal_key(&self.pending_shortcut, code, self.overlay_captures_input());
        self.pending_shortcut = translation.shortcut;
        match translation.input {
            NormalKeyInput::Overlay(key) => self.handle_overlay_key(key).await?,
            NormalKeyInput::CancelShortcut => {}
            NormalKeyInput::CancelOverlay => self.execute(Action::CancelOverlay).await?,
            NormalKeyInput::Action(action) => self.execute(action).await?,
            NormalKeyInput::Prefix => {}
            NormalKeyInput::Missing(label) => {
                self.set_warning(format!("invalid shortcut: {label}"));
            }
        }
        self.pending_shortcut_scroll = 0;
        Ok(())
    }

    pub(crate) async fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        self.handle_overlay_key_at_size(key, Size::new(80, 24))
            .await
    }

    async fn handle_overlay_key_at_size(
        &mut self,
        key: KeyEvent,
        terminal_size: Size,
    ) -> Result<()> {
        let overlay = if let Some(overlay) = self.overlay.take() {
            overlay
        } else if self.detail.is_active() {
            OverlayState::Detail
        } else {
            return Ok(());
        };

        match overlay {
            OverlayState::Onboarding { persist_on_exit } => {
                self.handle_onboarding_key(key, terminal_size, persist_on_exit)
                    .await?
            }
            OverlayState::Search(state) => self.handle_search_key(state, key).await?,
            OverlayState::RecurrenceHistory(state) => {
                self.handle_recurrence_history_key(*state, key).await?
            }
            OverlayState::Update(state) => {
                self.handle_update_overlay_key(state, key, terminal_size)
                    .await
            }
            OverlayState::Changelog(state) => self.handle_changelog_key(state, key, terminal_size),
            OverlayState::Command { mut state } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if !self.accept_command_input(&state).await? {
                        self.overlay = Some(OverlayState::Command { state });
                    }
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    self.complete_command_input(&mut state, key.code == KeyCode::BackTab);
                    self.overlay = Some(OverlayState::Command { state });
                }
                _ => {
                    state.input.handle_key(key);
                    state.reset_cycle();
                    self.overlay = Some(OverlayState::Command { state });
                }
            },
            overlay => {
                self.handle_generic_overlay_key(key, overlay, terminal_size)
                    .await?
            }
        }

        Ok(())
    }

    async fn handle_overlay_mouse(
        &mut self,
        overlay: OverlayState,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
        let was_add_task_picker = matches!(
            &overlay,
            OverlayState::Picker(state)
                if matches!(
                    state.intent,
                    PickerIntent::AddTaskProject | PickerIntent::AddTaskPriority
                )
        ) || matches!(
            &overlay,
            OverlayState::TagCombobox(state)
                if state.intent == TagComboboxIntent::AddTaskLabels
        );
        let outcome =
            crate::tui::overlay::handle_generic_overlay_mouse(overlay, mouse, terminal_size);
        self.apply_generic_overlay_outcome(outcome, false, false, was_add_task_picker)
            .await?;
        Ok(())
    }

    async fn handle_generic_overlay_key(
        &mut self,
        key: KeyEvent,
        overlay: OverlayState,
        terminal_size: Size,
    ) -> Result<()> {
        if let OverlayState::AttachmentPreview {
            attachment_id,
            scroll,
        } = &overlay
        {
            if key.code == KeyCode::Char('q') && key.modifiers.is_empty() {
                self.close_detail_session().await?;
                return Ok(());
            }
            if key.code == KeyCode::Char('D')
                && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
            {
                let attachment_id = attachment_id.clone();
                self.begin_delete_attachment(&attachment_id, *scroll);
                return Ok(());
            }
            if key.code == KeyCode::Char('o') && key.modifiers.is_empty() {
                let attachment_id = attachment_id.clone();
                self.open_attachment_externally(&attachment_id).await;
                self.overlay = Some(OverlayState::AttachmentPreview {
                    attachment_id,
                    scroll: *scroll,
                });
                return Ok(());
            }
            if key.code == KeyCode::Char('s') && key.modifiers.is_empty() {
                let attachment_id = attachment_id.clone();
                self.begin_save_attachment(&attachment_id, *scroll);
                return Ok(());
            }
            let next_attachment_id = match (key.code, key.modifiers) {
                (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => self
                    .move_attachment_preview_selection(attachment_id, 1)
                    .unwrap_or_else(|| attachment_id.clone()),
                (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => self
                    .move_attachment_preview_selection(attachment_id, -1)
                    .unwrap_or_else(|| attachment_id.clone()),
                _ => attachment_id.clone(),
            };
            if key.code == KeyCode::Esc {
                self.show_detail(*scroll);
            } else {
                self.overlay = Some(OverlayState::AttachmentPreview {
                    attachment_id: next_attachment_id,
                    scroll: *scroll,
                });
            }
            return Ok(());
        }
        let removes_add_task_image = matches!(
            &overlay,
            OverlayState::AddTask(state)
                if matches!(state.mode, AddTaskMode::Compose)
                    && state.focus == AddTaskStep::Images
        );
        if key.code == KeyCode::Char('D')
            && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
            && removes_add_task_image
        {
            self.overlay = Some(overlay);
            self.remove_selected_add_task_image();
            return Ok(());
        }
        if let OverlayState::Detail = overlay {
            let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
            if key.code == KeyCode::Esc
                && self
                    .detail
                    .state_mut()
                    .is_some_and(|detail| detail.clear_text_selection())
            {
                if let Some(detail) = self.detail.state_mut() {
                    detail.finish_text_drag();
                }
                self.show_detail(scroll);
                return Ok(());
            }
            if self.store.view_state.view == TaskView::Recurring {
                let recurrence_action = if key.modifiers.is_empty()
                    && self.pending_shortcut.is_empty()
                {
                    match key.code {
                        KeyCode::Char('e') => Some(Action::BeginEditRecurrenceTemplate),
                        KeyCode::Char('h') => Some(Action::ShowRecurrenceHistory),
                        KeyCode::Char('s') => Some(Action::StopRecurrence),
                        KeyCode::Char('p') => self.store.recurrence_detail.as_ref().map(|detail| {
                            if detail.series.state
                                == aven_core::recurrence::RecurrenceSeriesState::Paused
                            {
                                Action::ResumeRecurrence
                            } else {
                                Action::PauseRecurrence
                            }
                        }),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(action) = recurrence_action {
                    self.execute_selected_recurrence_action(action).await?;
                    return Ok(());
                }
                if key.code == KeyCode::Enter && key.modifiers.is_empty() {
                    self.open_recurrence_occurrence().await?;
                    return Ok(());
                }
            }
            if key.code == KeyCode::Char('q') && key.modifiers.is_empty() {
                self.close_detail_session().await?;
                return Ok(());
            }
            if key.code == KeyCode::Esc && !self.pending_shortcut.is_empty() {
                self.pending_shortcut.clear();
                self.pending_shortcut_scroll = 0;
                self.show_detail(scroll);
                return Ok(());
            }
            if key.code == KeyCode::Char('y')
                && key.modifiers.is_empty()
                && self
                    .detail
                    .state()
                    .and_then(|detail| detail.text_selection())
                    .is_some()
            {
                self.pending_shortcut.clear();
                self.pending_shortcut_scroll = 0;
                self.copy_detail_text_selection();
                self.show_detail(scroll);
                return Ok(());
            }
            let had_detail_focus = self
                .detail
                .state()
                .and_then(|detail| detail.focused_target())
                .is_some();
            let selected_target = self.selected_detail_focus_target(terminal_size);
            if had_detail_focus && selected_target.is_none() {
                if let Some(detail) = self.detail.state_mut() {
                    detail.set_focused_target(None);
                }
                self.show_detail(scroll);
                return Ok(());
            }
            if !self.pending_shortcut.is_empty()
                && self.handle_focused_detail_shortcut(key, scroll).await?
            {
                return Ok(());
            }
            if (!self.pending_shortcut.is_empty() || key.code == KeyCode::Char('g'))
                && let Some(outcome) = self.handle_detail_shortcut(key, scroll).await?
            {
                self.overlay = outcome;
                return Ok(());
            }
            if let Some(selected_target) = selected_target {
                let mut focused_scroll = scroll;
                match (key.code, key.modifiers) {
                    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                        self.move_detail_focus_selection(1, terminal_size);
                        focused_scroll = self.detail_focus_scroll(scroll, terminal_size);
                    }
                    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                        self.move_detail_focus_selection(-1, terminal_size);
                        focused_scroll = self.detail_focus_scroll(scroll, terminal_size);
                    }
                    (KeyCode::Char('e'), KeyModifiers::NONE) => {
                        if let DetailTargetId::Note { note_id } = &selected_target {
                            self.begin_edit_note(note_id, scroll);
                            return Ok(());
                        }
                    }
                    (KeyCode::Char('D'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                        match &selected_target {
                            DetailTargetId::Note { note_id } => {
                                self.begin_delete_note(note_id, scroll);
                                return Ok(());
                            }
                            DetailTargetId::Attachment { attachment_id } => {
                                self.begin_delete_attachment(attachment_id, scroll);
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                    (KeyCode::Char('o'), KeyModifiers::NONE) => {
                        if let DetailTargetId::Attachment { attachment_id } = &selected_target {
                            self.open_attachment_externally(attachment_id).await;
                        }
                    }
                    (KeyCode::Char('s'), KeyModifiers::NONE) => {
                        if let DetailTargetId::Attachment { attachment_id } = &selected_target {
                            self.begin_save_attachment(attachment_id, scroll);
                            return Ok(());
                        }
                    }
                    (KeyCode::Enter, KeyModifiers::NONE) => match selected_target {
                        DetailTargetId::Task { task_id, .. } => {
                            self.open_detail_task(&task_id, scroll).await;
                            return Ok(());
                        }
                        DetailTargetId::Note { .. } => {}
                        DetailTargetId::Attachment { attachment_id } => {
                            if self.detail_attachment_supports_inline_preview(&attachment_id) {
                                self.open_detail_attachment(attachment_id, scroll);
                            } else {
                                self.open_attachment_externally(&attachment_id).await;
                                self.show_detail(scroll);
                            }
                            return Ok(());
                        }
                        DetailTargetId::Expand { section } => {
                            self.activate_detail_disclosure(section, terminal_size);
                            focused_scroll = self.detail_focus_scroll(scroll, terminal_size);
                        }
                    },
                    (KeyCode::Esc, _) => {
                        if let Some(detail) = self.detail.state_mut() {
                            detail.set_focused_target(None);
                        }
                    }
                    (KeyCode::Tab, KeyModifiers::NONE) => {
                        self.focus_detail_section(false, terminal_size);
                        focused_scroll = self.detail_focus_scroll(scroll, terminal_size);
                    }
                    (KeyCode::BackTab, KeyModifiers::NONE | KeyModifiers::SHIFT)
                    | (KeyCode::Tab, KeyModifiers::SHIFT) => {
                        self.focus_detail_section(true, terminal_size);
                        focused_scroll = self.detail_focus_scroll(scroll, terminal_size);
                    }
                    _ => {
                        if self.handle_focused_detail_shortcut(key, scroll).await? {
                            return Ok(());
                        }
                    }
                }
                self.show_detail(focused_scroll);
                return Ok(());
            }
            let section_direction = match (key.code, key.modifiers) {
                (KeyCode::Tab, KeyModifiers::NONE) => Some(false),
                (KeyCode::BackTab, KeyModifiers::NONE | KeyModifiers::SHIFT)
                | (KeyCode::Tab, KeyModifiers::SHIFT) => Some(true),
                _ => None,
            };
            if let Some(reverse) = section_direction
                && self.focus_detail_section(reverse, terminal_size)
            {
                let focused_scroll = self.detail_focus_scroll(scroll, terminal_size);
                self.show_detail(focused_scroll);
                return Ok(());
            }
            let inline_images = self.inline_image_context();
            if let Some(reverse) = section_direction {
                let target_scroll = self
                    .detail_document_for_query(terminal_size)
                    .map(|document| document.section_scroll_target(reverse))
                    .unwrap_or(scroll);
                self.show_detail(target_scroll);
                return Ok(());
            }

            if let Some(outcome) = self.handle_detail_shortcut(key, scroll).await? {
                self.overlay = outcome;
                return Ok(());
            }

            if let Some(delta) = detail_task_delta(key) {
                self.select_detail_task(delta);
                self.show_detail(0);
                return Ok(());
            }

            if key.code == KeyCode::Esc {
                self.navigate_back_from_detail().await?;
                return Ok(());
            }

            let task = self.store.selected_task(self.list.selected_task());
            let scroll = if let Some(document) = self.detail_document_for_query(terminal_size) {
                handle_detail_scroll_key_with_cap(
                    key,
                    scroll,
                    terminal_size.height,
                    document.scroll_cap(),
                )
            } else {
                handle_detail_scroll_key_with_images(
                    key,
                    scroll,
                    terminal_size.width,
                    terminal_size.height,
                    task,
                    inline_images.as_ref(),
                )
            };
            self.show_detail(scroll);
            return Ok(());
        }

        if matches!(
            &overlay,
            OverlayState::AddTask(state)
                if state.mode == AddTaskMode::Compose
                    && state.focus.is_metadata()
                    && key.code == KeyCode::Enter
        ) {
            self.overlay = Some(overlay);
            self.open_focused_add_task_control();
            return Ok(());
        }

        let had_add_task_status_prefix = self.pending_shortcut.has_add_task_status_prefix();
        if let Some(status) = self.pending_shortcut.take_add_task_status_request(key) {
            if let OverlayState::AddTask(state) = &overlay {
                if self.capture_add_task_state(state) {
                    self.overlay = Some(overlay);
                    self.set_add_task_status(status);
                }
            } else {
                self.overlay = Some(overlay);
            }
            return Ok(());
        }
        if had_add_task_status_prefix {
            self.pending_shortcut.clear();
            self.overlay = Some(overlay);
            if key.code != KeyCode::Esc {
                self.set_warning("invalid status shortcut");
            }
            return Ok(());
        }

        let had_add_task_priority_prefix = self.pending_shortcut.has_add_task_priority_prefix();
        if let Some(priority) = self.pending_shortcut.take_add_task_priority_request(key) {
            if let OverlayState::AddTask(state) = &overlay {
                if self.capture_add_task_state(state) {
                    self.overlay = Some(overlay);
                    self.set_add_task_priority(priority);
                }
            } else {
                self.overlay = Some(overlay);
            }
            return Ok(());
        }
        if had_add_task_priority_prefix {
            self.pending_shortcut.clear();
            self.overlay = Some(overlay);
            if key.code != KeyCode::Esc {
                self.set_warning("invalid priority shortcut");
            }
            return Ok(());
        }

        if self.pending_shortcut.take_editor_open_request(key) {
            match &overlay {
                OverlayState::MultilineInput(state) if state.intent.is_description_edit() => {
                    self.open_description_external_editor(state.clone());
                }
                OverlayState::MultilineInput(state)
                    if matches!(state.intent, MultilineIntent::EditNote { .. }) =>
                {
                    self.open_note_external_editor(state.clone());
                }
                OverlayState::AddTask(state) if state.focus == AddTaskStep::Description => {
                    if self.capture_add_task_state(state) {
                        self.open_add_task_description_editor();
                    }
                }
                _ => self.overlay = Some(overlay),
            }
            return Ok(());
        }

        if is_editor_prefix_key(key)
            && matches!(
                &overlay,
                OverlayState::MultilineInput(state)
                    if state.intent.supports_external_editor()
            )
        {
            self.pending_shortcut.begin_editor_prefix();
            self.overlay = Some(overlay);
            return Ok(());
        }

        if let OverlayState::AddTask(state) = &overlay {
            if is_editor_prefix_key(key) {
                if state.focus == AddTaskStep::Description {
                    self.pending_shortcut.begin_editor_prefix();
                }
                self.overlay = Some(overlay);
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('a')
                && state.mode == AddTaskMode::Compose
            {
                self.overlay = Some(overlay);
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    let mut editor = state.schedule_editor(ScheduleEditorField::Available);
                    editor.mode = ScheduleEditorMode::Once;
                    editor.focus = ScheduleEditorField::Available;
                    editor.refresh();
                    state.mode = AddTaskMode::Schedule(editor);
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('u')
                && state.mode == AddTaskMode::Compose
            {
                self.overlay = Some(overlay);
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    let mut editor = state.schedule_editor(ScheduleEditorField::Due);
                    editor.mode = ScheduleEditorMode::Once;
                    editor.focus = ScheduleEditorField::Due;
                    editor.refresh();
                    state.mode = AddTaskMode::Schedule(editor);
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
                let return_focus = state.focus;
                self.overlay = Some(overlay);
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    state.focus = AddTaskStep::Project;
                }
                self.open_focused_add_task_control();
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    state.focus = return_focus;
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
                let return_focus = state.focus;
                self.overlay = Some(overlay);
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    state.focus = AddTaskStep::Labels;
                }
                self.open_focused_add_task_control();
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    state.focus = return_focus;
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
                self.pending_shortcut.begin_add_task_priority_prefix();
                self.overlay = Some(overlay);
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
                self.pending_shortcut.begin_add_task_status_prefix();
                self.overlay = Some(overlay);
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
                let title = state.title.text.clone();
                let description = state.description.lines.join("\n");
                if self.capture_add_task_state(state) {
                    self.submit_add_task_title_natural(title, description)
                        .await?;
                }
                return Ok(());
            }
        }

        let scroll_cap = match &overlay {
            OverlayState::AddTask(state) if matches!(state.mode, AddTaskMode::Help { .. }) => {
                composer_help_scroll_cap(
                    terminal_size.height,
                    self.intake.view().add_task_only,
                    state.schedule_expanded,
                )
            }
            OverlayState::DetailHelp { .. } => detail_help_scroll_cap(terminal_size.height),
            OverlayState::DatabaseStats { .. } => database_stats_scroll_cap(terminal_size.height),
            OverlayState::Changelog(state) => {
                crate::tui::changelog::changelog_scroll_cap(&state.markdown, terminal_size)
            }
            _ => help_scroll_cap(terminal_size.height),
        };
        let was_detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
        let was_add_task_description_editor = matches!(
            &overlay,
            OverlayState::MultilineInput(state)
                if state.intent == MultilineIntent::AddTaskDescription
        );
        let was_add_task_picker = matches!(
            &overlay,
            OverlayState::Picker(state)
                if matches!(
                    state.intent,
                    PickerIntent::AddTaskProject | PickerIntent::AddTaskPriority
                )
        ) || matches!(
            &overlay,
            OverlayState::TagCombobox(state)
                if state.intent == TagComboboxIntent::AddTaskLabels
        );
        let outcome = crate::tui::overlay::handle_generic_overlay_key(key, overlay, scroll_cap);
        self.apply_generic_overlay_outcome(
            outcome,
            was_detail_help,
            was_add_task_description_editor,
            was_add_task_picker,
        )
        .await
    }

    async fn apply_generic_overlay_outcome(
        &mut self,
        outcome: OverlayOutcome,
        was_detail_help: bool,
        was_add_task_description_editor: bool,
        was_add_task_picker: bool,
    ) -> Result<()> {
        match outcome {
            OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
            OverlayOutcome::Cancelled if was_detail_help => {}
            OverlayOutcome::Cancelled if was_add_task_description_editor || was_add_task_picker => {
                self.begin_add_task_step()
            }
            OverlayOutcome::Cancelled if self.intake.view().add_task_only => {
                self.should_quit = true
            }
            OverlayOutcome::Cancelled => self.cancel_authoring_overlay(),
            OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
        }
        Ok(())
    }

    pub(in crate::tui) fn detail_focus_allows_action(&self, action: Action) -> bool {
        let Some(target) = self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
        else {
            return true;
        };
        let policy = crate::tui::event::CommandContext::Detail
            .commands()
            .find(|command| command.action == action)
            .map(|command| command.detail_focus)
            .unwrap_or(DetailFocusPolicy::ParentTask);
        match policy {
            DetailFocusPolicy::Global => true,
            DetailFocusPolicy::ParentTask => false,
            DetailFocusPolicy::EpicChild => matches!(
                target,
                DetailTargetId::Task {
                    section: DetailSection::EpicChildren,
                    ..
                }
            ),
        }
    }

    pub(in crate::tui) fn detail_focus_warning(&self) -> &'static str {
        match self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
        {
            Some(DetailTargetId::Task {
                section: DetailSection::EpicChildren,
                ..
            }) => "leave epic child focus before using that command",
            Some(DetailTargetId::Task { .. }) => {
                "leave relationship focus before using that command"
            }
            Some(DetailTargetId::Note { .. }) => "leave note focus before using that command",
            Some(DetailTargetId::Attachment { .. }) => {
                "leave attachment focus before using that command"
            }
            Some(DetailTargetId::Expand { .. }) => {
                "leave relationship disclosure focus before using that command"
            }
            None => "leave detail focus before using that command",
        }
    }

    async fn handle_focused_detail_shortcut(&mut self, key: KeyEvent, scroll: u16) -> Result<bool> {
        let Some(target) = self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
            .cloned()
        else {
            return Ok(false);
        };
        if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
            return Ok(false);
        }
        match self.pending_shortcut.resolve_detail(key) {
            DetailShortcutResolution::Action(Action::GoBack) => {
                self.pending_shortcut_scroll = 0;
                self.navigate_back_from_detail().await?;
                Ok(true)
            }
            DetailShortcutResolution::Action(action) => {
                self.execute_focused_detail_action(action, &target, scroll)
                    .await?;
                Ok(true)
            }
            DetailShortcutResolution::Prefix => {
                self.pending_shortcut_scroll = 0;
                self.show_detail(scroll);
                Ok(true)
            }
            DetailShortcutResolution::MissingAfterPrefix(label) => {
                self.pending_shortcut_scroll = 0;
                self.set_warning(format!("invalid shortcut: {label}"));
                self.show_detail(scroll);
                Ok(true)
            }
            DetailShortcutResolution::PassThrough => Ok(false),
        }
    }

    fn action_supports_related_task(action: Action) -> bool {
        matches!(
            action,
            Action::SetStatus(_)
                | Action::SetPriority(_)
                | Action::CyclePriority(_)
                | Action::CopyShortRef
                | Action::CopyDurableRef
                | Action::CopyTaskTitle
                | Action::CopyTaskDescription
                | Action::CopyTaskText
                | Action::CopyTaskNotes
                | Action::BeginEditTitle
                | Action::BeginEditDescription
                | Action::BeginEditProject
                | Action::BeginEditPriority
                | Action::BeginEditAvailability
                | Action::BeginEditDue
                | Action::BeginEditLabels
                | Action::Delete
                | Action::Restore
                | Action::BeginStatusPicker
                | Action::BeginAddNote
                | Action::BeginAddDependency
        )
    }

    async fn execute_focused_detail_action(
        &mut self,
        action: Action,
        target: &DetailTargetId,
        scroll: u16,
    ) -> Result<()> {
        self.pending_shortcut_scroll = 0;
        if let Some(detail) = self.detail.state_mut() {
            detail.set_scroll(scroll);
        }

        if matches!(action, Action::Undo | Action::ReturnToLastChange) {
            self.execute(action).await?;
            return Ok(());
        }

        let DetailTargetId::Task { section, task_id } = target else {
            self.set_warning(self.detail_focus_warning());
            self.show_detail(scroll);
            return Ok(());
        };

        if action == Action::RemoveEpicChild {
            if *section == DetailSection::EpicChildren {
                self.execute(action).await?;
            } else {
                self.set_warning("This relationship cannot be removed with that command");
                self.show_detail(scroll);
            }
            return Ok(());
        }

        if !Self::action_supports_related_task(action) {
            self.set_warning("Open the related task before using that command");
            self.show_detail(scroll);
            return Ok(());
        }

        let Some(anchor_index) = self.list.selected_task() else {
            self.set_warning("detail task is unavailable");
            return Ok(());
        };
        let Some(anchor) = self.store.tasks.get(anchor_index).cloned() else {
            self.set_warning("detail task is unavailable");
            return Ok(());
        };
        let Some(item) = self.store.load_task_item(task_id).await? else {
            self.set_warning("linked task is unavailable");
            return Ok(());
        };
        self.detail_command_selection = Some(
            crate::tui::task_selection::TaskSelection::for_detail_target(
                item,
                &anchor,
                anchor_index,
            ),
        );
        let result = self.execute(action).await;
        self.detail_command_selection = None;
        result
    }

    async fn handle_detail_shortcut(
        &mut self,
        key: KeyEvent,
        scroll: u16,
    ) -> Result<Option<Option<OverlayState>>> {
        if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
            return Ok(None);
        }

        match self.pending_shortcut.resolve_detail(key) {
            DetailShortcutResolution::Action(Action::GoBack) => {
                self.pending_shortcut_scroll = 0;
                self.navigate_back_from_detail().await?;
                Ok(Some(self.overlay.take()))
            }
            DetailShortcutResolution::Action(action) => {
                self.pending_shortcut_scroll = 0;
                if !self.detail_focus_allows_action(action) {
                    self.set_warning(self.detail_focus_warning());
                    if let Some(detail) = self.detail.state_mut() {
                        detail.set_scroll(scroll);
                    }
                    self.show_detail(scroll);
                    return Ok(Some(self.overlay.take()));
                }
                if let Some(detail) = self.detail.state_mut() {
                    detail.set_scroll(scroll);
                }
                self.execute(action).await?;
                Ok(Some(self.overlay.take()))
            }
            DetailShortcutResolution::Prefix => {
                self.pending_shortcut_scroll = 0;
                Ok(Some(None))
            }
            DetailShortcutResolution::MissingAfterPrefix(label) => {
                self.pending_shortcut_scroll = 0;
                self.set_warning(format!("invalid shortcut: {label}"));
                Ok(Some(None))
            }
            DetailShortcutResolution::PassThrough => Ok(None),
        }
    }

    async fn accept_command_input(&mut self, state: &CommandState) -> Result<bool> {
        let input = state.input.as_str();
        match lookup_command_spec_for(state.context, input) {
            CommandSpecLookup::Found(command) => {
                self.pending_shortcut.clear();
                let focused_target = (state.context == crate::tui::event::CommandContext::Detail)
                    .then(|| {
                        self.detail
                            .state()
                            .and_then(|detail| detail.focused_target())
                            .cloned()
                    })
                    .flatten();
                let routes_to_related_task =
                    matches!(&focused_target, Some(DetailTargetId::Task { .. }))
                        && Self::action_supports_related_task(command.action);
                if focused_target.is_some()
                    && !routes_to_related_task
                    && !self.detail_focus_allows_action(command.action)
                {
                    self.set_warning(self.detail_focus_warning());
                    return Ok(true);
                }
                if let Some(unavailable) = state
                    .unavailable
                    .iter()
                    .find(|override_| override_.action == command.action)
                {
                    self.set_warning(format!(
                        ":{} is disabled: {}",
                        command.name, unavailable.reason
                    ));
                    return Ok(true);
                }
                if routes_to_related_task {
                    let target = focused_target.as_ref().expect("focused task target");
                    let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
                    self.execute_focused_detail_action(command.action, target, scroll)
                        .await?;
                    return Ok(true);
                }
                if command.action.recurrence_kind().is_some() {
                    self.execute_targeted_recurrence_action(state.target.clone(), command.action)
                        .await?;
                } else {
                    self.execute(command.action).await?;
                }
                Ok(true)
            }
            CommandSpecLookup::Empty => {
                self.set_info("empty command");
                Ok(false)
            }
            CommandSpecLookup::Ambiguous => {
                self.set_warning(format!("ambiguous command: {}", input.trim()));
                Ok(false)
            }
            CommandSpecLookup::Missing => {
                self.set_warning(format!("unknown command: {}", input.trim()));
                Ok(false)
            }
        }
    }

    fn complete_command_input(&mut self, state: &mut CommandState, reverse: bool) {
        let cycle_input = state
            .cycle_input
            .clone()
            .unwrap_or_else(|| state.input.text.clone());
        let options = command_cycle_options_for(state.context, &cycle_input);
        if options.len() > 1 {
            state.cycle_index = if state.cycle_input.is_some() {
                if reverse {
                    state
                        .cycle_index
                        .checked_sub(1)
                        .unwrap_or(options.len().saturating_sub(1))
                } else {
                    (state.cycle_index + 1) % options.len()
                }
            } else if reverse {
                options.len().saturating_sub(1)
            } else {
                0
            };
            state.cycle_input = Some(cycle_input);
            let completion = options[state.cycle_index].to_string();
            state.input.text = completion;
            state.input.cursor = state.input.text.len();
            state.highlighted = Some(state.input.text.clone());
            return;
        }

        let highlighted = state.highlighted.clone();
        state.reset_cycle();
        state.highlighted = highlighted;
        match complete_command_for(state.context, state.input.as_str()) {
            CommandCompletion::Completed(completion) => {
                state.input.text = completion;
                state.input.cursor = state.input.text.len();
                state.highlighted = Some(state.input.text.clone());
            }
            CommandCompletion::Empty => self.set_info("type a command prefix to complete"),
            CommandCompletion::Missing => self.set_warning(format!(
                "no command matches: {}",
                state.input.as_str().trim()
            )),
            CommandCompletion::Unchanged => {
                if let Some(completion) = options.first() {
                    state.input.text = (*completion).to_string();
                    state.input.cursor = state.input.text.len();
                    state.highlighted = Some(state.input.text.clone());
                } else {
                    self.set_info("no further completion");
                }
            }
        }
    }

    pub(super) fn toggle_help_at_height(&mut self, _terminal_height: u16) {
        match self.overlay {
            Some(OverlayState::Help { .. }) => self.overlay = None,
            Some(OverlayState::DetailHelp { .. }) => self.overlay = None,
            None if self.detail.is_active() => {
                self.overlay = Some(OverlayState::DetailHelp { scroll: 0 })
            }
            _ => self.overlay = Some(OverlayState::Help { scroll: 0 }),
        }
    }
}

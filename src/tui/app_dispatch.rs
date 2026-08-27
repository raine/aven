use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};

use std::time::{Duration, Instant};

use crate::tui::app::{App, DetailSection, DetailTargetId, Focus, FooterChoiceMode};
use crate::tui::authoring::AddTaskStep;
use crate::tui::detail_session::DetailTargetActivation;
use crate::tui::event::{Action, CommandHandler, DetailFocusPolicy};
use crate::tui::input::key::{
    ImagePasteTarget, KeyInput, KeyRouteState, NormalKeyInput, route_key,
    route_normal_key_in_domain,
};
use crate::tui::input::mouse::{
    MouseInput, PointerEvent, TaskSurfaceView, route_mouse, route_task_surface,
};
use crate::tui::navigation::{
    detail_scroll_with_delta_with_images, detail_task_delta, handle_detail_scroll_key_with_cap,
    handle_detail_scroll_key_with_images, next_index, scroll_with_delta,
};
use crate::tui::overlay::{
    AddTaskMode, CommandState, ConfirmIntent, MultilineIntent, OverlayOutcome, OverlayState,
    PickerIntent, ScheduleEditorField, ScheduleEditorMode, TagComboboxIntent,
};
use crate::tui::platform::{copy_to_clipboard, is_editor_prefix_key, open_url_in_default_browser};
use crate::tui::shortcut_buffer::DetailShortcutResolution;
use crate::tui::store::TaskQuery;
use crate::tui::ui::{
    composer_help_scroll_cap, database_stats_scroll_cap, detail_copy_target_at,
    detail_help_scroll_cap, help_scroll_cap, prefix_hint_scroll_cap, task_at_position,
    task_status_at_position,
};

#[derive(Clone)]
struct FocusedRelationship {
    section: DetailSection,
    task_id: crate::ids::TaskId,
    display_ref: String,
    title: String,
}

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
        self.list.cancel_column_drag();
        let input = route_key(
            key,
            KeyRouteState {
                footer_choice: self.footer_choice.is_some(),
                shortcut_pending: !self.pending_shortcut.is_empty(),
                prefix_hints: self.prefix_hints_active(),
                overlay_captures: self.overlay_captures_input()
                    || (self.detail.is_active() && self.overlay.is_none()),
                detail_overlay: self.detail.is_active() && self.overlay.is_none(),
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
        let shortcut_scroll = self.shortcut_overlay_scroll(mouse.kind);
        let task_list_wheel = self.routes_wheel_to_task_list(mouse.kind, terminal_size);
        let previous_selection = self.list.selected_task();
        if mouse.kind != MouseEventKind::Moved {
            self.handle_mouse(mouse, terminal_size).await?;
            if let Some(previous_scroll) = shortcut_scroll {
                return Ok(self.shortcut_overlay_scroll(mouse.kind) != Some(previous_scroll));
            }
            return Ok(!task_list_wheel || self.list.selected_task() != previous_selection);
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
        if mouse.kind == MouseEventKind::Moved && self.overlay.is_some() {
            self.handle_detail_mouse_move(mouse, terminal_size);
        }
        if matches!(self.overlay, Some(OverlayState::RecurrenceHistory(_))) {
            return self.dispatch_overlay_mouse(mouse, terminal_size).await;
        }
        let input = route_mouse(mouse.kind, self.prefix_hints_active());
        if let MouseInput::PrefixScroll(delta) = input {
            self.dispatch_prefix_hint_scroll(delta, terminal_size);
            return Ok(());
        }
        if self.overlay.is_some() {
            self.list.clear_task_click();
            self.list.cancel_column_drag();
            return self.dispatch_overlay_mouse(mouse, terminal_size).await;
        }
        match input {
            MouseInput::PrefixScroll(_) => unreachable!("prefix scroll was handled"),
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
                    .handle_detail_target_mouse_click(mouse, terminal_size)
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
                if self.list.column_drag().is_some() {
                    let target_lane = crate::tui::ui::column_lane_body_at_position(
                        &self.store,
                        self.list.table_state(),
                        self.task_area_for_mouse(terminal_size),
                        mouse.column,
                        mouse.row,
                    );
                    self.list
                        .update_column_drag(target_lane, (mouse.column, mouse.row));
                } else {
                    self.update_detail_text_selection(mouse, terminal_size);
                }
                return Ok(());
            }
            MouseInput::DetailRelease => {
                if let Some(drag) = self.list.take_column_drag() {
                    if drag.is_active() {
                        let target_lane = crate::tui::ui::column_lane_body_at_position(
                            &self.store,
                            self.list.table_state(),
                            self.task_area_for_mouse(terminal_size),
                            mouse.column,
                            mouse.row,
                        );
                        if let Some(target_lane) = target_lane
                            && target_lane != drag.origin_lane
                        {
                            self.drop_task_on_column(drag.task_id, target_lane).await?;
                        }
                    }
                } else if let Some(detail) = self.detail.state_mut() {
                    detail.finish_text_drag();
                }
                return Ok(());
            }
            MouseInput::PointerMove => {
                self.handle_detail_mouse_move(mouse, terminal_size);
                return Ok(());
            }
            MouseInput::StatusPress => {
                if self.store.view_state.query == TaskQuery::Recurring {
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
            && terminal_size.width >= crate::tui::ui::MIN_TUI_WIDTH
            && terminal_size.height >= crate::tui::ui::MIN_TUI_HEIGHT
            && self.detail.is_inactive()
            && self.footer_choice.is_none()
        {
            let marked_task_count = self.bulk_scope_marked_task_count();
            let terminal_area = Rect::new(0, 0, terminal_size.width, terminal_size.height);
            if let Some(action) = crate::tui::ui::bulk_footer_action_at(
                crate::tui::ui::footer_area(terminal_area),
                marked_task_count,
                mouse.column,
                mouse.row,
            ) {
                self.list.clear_task_click();
                self.execute(action).await?;
                return Ok(());
            }
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

        if self.detail.is_active() && self.overlay.is_none() {
            let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
            self.handle_detail_mouse_click(mouse, terminal_size, scroll)
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
                crate::tui::ui::HeaderTarget::Query { column } => {
                    self.show_view_menu(column, mouse.row);
                    Ok(())
                }
                crate::tui::ui::HeaderTarget::Layout => {
                    self.toggle_layout();
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
                let is_double_click = self.list.register_task_click(
                    hit.task_id.clone(),
                    hit.viewport_row,
                    Instant::now(),
                );
                if is_double_click {
                    self.list.cancel_column_drag();
                    self.show_detail(0);
                } else if self.store.view_state.is_columns()
                    && let Some(origin_lane) =
                        self.store.tasks.get(hit.task_index).and_then(|item| {
                            crate::tui::columns::lane_index_for_status(
                                &self.store.task_columns,
                                item.task.status,
                            )
                        })
                {
                    self.list.begin_column_drag(
                        hit.task_id,
                        origin_lane,
                        (mouse.column, mouse.row),
                    );
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
        let hit = if self.store.view_state.is_columns() {
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
        self.set_mutation_success(mutation.message.message);
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

    async fn handle_detail_target_mouse_click(
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
        let document = self.detail_document_for_query(terminal_size);
        if let Some(url) = document
            .as_ref()
            .and_then(|document| document.link_at_position(mouse.column, mouse.row))
        {
            match crate::tui::platform::open_url_in_default_browser(&url) {
                Ok(()) => self.set_success("opened link in browser"),
                Err(error) => self.set_error(format!("could not open link: {error}")),
            }
            return Ok(true);
        }
        let hit =
            document.and_then(|document| document.target_at_position(mouse.column, mouse.row));
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

        let Some(item) = self.store.selected_task(self.list.selected_task()) else {
            return Ok(false);
        };
        let Some((target, _column, _row)) = crate::tui::ui::detail_metadata_target_at(
            item,
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
            crate::tui::ui::DetailMetadataTarget::Project => self.begin_edit_project(),
            crate::tui::ui::DetailMetadataTarget::Status => self.begin_status_picker(),
            crate::tui::ui::DetailMetadataTarget::Priority => self.begin_edit_priority(),
            crate::tui::ui::DetailMetadataTarget::Labels => self.begin_edit_labels(),
            crate::tui::ui::DetailMetadataTarget::Availability => {
                self.begin_edit_availability();
            }
            crate::tui::ui::DetailMetadataTarget::Due => self.begin_edit_due(),
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

    fn shortcut_overlay_scroll(&self, kind: MouseEventKind) -> Option<u16> {
        if !matches!(kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            return None;
        }
        match self.overlay.as_ref() {
            Some(OverlayState::Help { scroll } | OverlayState::DetailHelp { scroll }) => {
                Some(*scroll)
            }
            _ => None,
        }
    }

    fn routes_wheel_to_task_list(&self, kind: MouseEventKind, terminal_size: Size) -> bool {
        matches!(kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp)
            && !self.prefix_hints_active()
            && self.overlay.is_none()
            && terminal_size.width >= 70
            && terminal_size.height >= 18
            && !self.detail_underlay()
            && self.list.focus() == Focus::Tasks
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

        let next = if self.store.view_state.is_columns() {
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
            self.current_routing_domain(),
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
        false
    }

    fn overlay_captures_input(&self) -> bool {
        self.overlay
            .as_ref()
            .is_some_and(OverlayState::captures_input)
    }

    pub(crate) async fn handle_normal_key(&mut self, code: KeyCode) -> Result<()> {
        let domain = self.current_routing_domain();
        let translation = route_normal_key_in_domain(
            &self.pending_shortcut,
            code,
            self.overlay_captures_input(),
            &self.command_catalog,
            domain,
        );
        self.pending_shortcut = translation.shortcut;
        match translation.input {
            NormalKeyInput::Overlay(key) => self.handle_overlay_key(key).await?,
            NormalKeyInput::CancelShortcut => {}
            NormalKeyInput::CancelOverlay => self.execute(Action::CancelOverlay).await?,
            NormalKeyInput::Command(handler) => self.execute_command_handler(handler).await?,
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
                    if let Some(command) = state
                        .highlighted
                        .and_then(|row| state.candidates.get(row))
                        .and_then(|candidate| state.catalog.command(candidate.index))
                    {
                        state.input.text = command.name().to_string();
                        state.input.cursor = state.input.text.len();
                    }
                    if !self.accept_command_input(&state).await? {
                        self.overlay = Some(OverlayState::Command { state });
                    }
                }
                KeyCode::Down | KeyCode::Up => {
                    self.move_command_selection(&mut state, key.code == KeyCode::Up);
                    self.overlay = Some(OverlayState::Command { state });
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    self.complete_command_input(&mut state, key.code == KeyCode::BackTab);
                    self.overlay = Some(OverlayState::Command { state });
                }
                _ => {
                    state.input.handle_key(key);
                    state.reset_cycle();
                    state.refresh_candidates();
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

    async fn dispatch_overlay_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
        let detail_focus = self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
            .cloned();
        let context = crate::tui::overlay::OverlayMouseContext {
            add_task_only: self.intake.view().add_task_only,
            detail_help_scroll_cap: detail_help_scroll_cap(
                terminal_size.height,
                detail_focus.as_ref(),
            ),
        };
        let Some(overlay) = self.overlay.take() else {
            return Ok(());
        };
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
            crate::tui::overlay::dispatch_overlay_mouse(overlay, mouse, terminal_size, context);
        match outcome {
            crate::tui::overlay::OverlayMouseOutcome::Retained(overlay) => {
                self.overlay = Some(overlay)
            }
            crate::tui::overlay::OverlayMouseOutcome::Closed => {}
            crate::tui::overlay::OverlayMouseOutcome::Cancelled => {
                self.apply_generic_overlay_outcome(
                    OverlayOutcome::Cancelled,
                    false,
                    false,
                    was_add_task_picker,
                )
                .await?;
            }
            crate::tui::overlay::OverlayMouseOutcome::Submitted(submit) => {
                self.handle_overlay_submit(submit).await?;
            }
            crate::tui::overlay::OverlayMouseOutcome::OpenUrl {
                overlay,
                url,
                error_context,
            } => {
                self.overlay = Some(overlay);
                if let Err(error) = open_url_in_default_browser(&url) {
                    self.set_warning(format!("{error_context}: {error:#}"));
                }
            }
            crate::tui::overlay::OverlayMouseOutcome::OpenAddTaskControl(overlay) => {
                self.overlay = Some(overlay);
                self.open_focused_add_task_control();
            }
            crate::tui::overlay::OverlayMouseOutcome::UpdateAction(state) => {
                self.handle_update_overlay_key(
                    state,
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    terminal_size,
                )
                .await;
            }
            crate::tui::overlay::OverlayMouseOutcome::RecurrenceHistoryAction { state, action } => {
                self.run_recurrence_history_action(state, action).await?
            }
            crate::tui::overlay::OverlayMouseOutcome::Warning { overlay, message } => {
                self.overlay = Some(overlay);
                self.set_warning(message);
            }
        }
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
            if self.store.view_state.query == TaskQuery::Recurring
                && key.code == KeyCode::Enter
                && key.modifiers.is_empty()
            {
                self.open_recurrence_occurrence().await?;
                return Ok(());
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
            if self.pending_shortcut.is_empty()
                && self.command_catalog.custom_shortcut_starts_with(
                    crate::tui::event::CommandContext::Detail,
                    &[key.code],
                )
                && self.handle_focused_detail_shortcut(key, scroll).await?
            {
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
                    (KeyCode::Char('s'), KeyModifiers::NONE)
                        if matches!(&selected_target, DetailTargetId::Attachment { .. }) =>
                    {
                        let DetailTargetId::Attachment { attachment_id } = &selected_target else {
                            unreachable!("guarded attachment target");
                        };
                        self.begin_save_attachment(attachment_id, scroll);
                        return Ok(());
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
                self.select_detail_task(delta).await?;
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
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
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
            OverlayState::DetailHelp { .. } => detail_help_scroll_cap(
                terminal_size.height,
                self.detail
                    .state()
                    .and_then(|detail| detail.focused_target()),
            ),
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

    fn focused_relationship(&self) -> Option<FocusedRelationship> {
        let DetailTargetId::Task { section, task_id } = self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())?
        else {
            return None;
        };
        let item = self.store.selected_task(self.list.selected_task())?;
        if *section == DetailSection::Related {
            let link = item
                .related
                .iter()
                .find(|link| (!link.deleted || item.task.deleted) && link.task_id == *task_id)?;
            return Some(FocusedRelationship {
                section: *section,
                task_id: link.task_id.clone(),
                display_ref: link.display_ref.clone(),
                title: link.title.clone(),
            });
        }
        let link = match section {
            DetailSection::EpicParent => item
                .epic_parent
                .as_ref()
                .filter(|link| link.task_id == *task_id),
            DetailSection::EpicChildren => item
                .epic_children
                .iter()
                .find(|link| link.task_id == *task_id)
                .or_else(|| {
                    self.detail
                        .state()
                        .and_then(|detail| detail.removed_epic_child())
                        .map(|removed| &removed.child)
                        .filter(|link| link.task_id == *task_id)
                }),
            DetailSection::DependsOn => {
                item.depends_on.iter().find(|link| link.task_id == *task_id)
            }
            DetailSection::Blocks => item.blocks.iter().find(|link| link.task_id == *task_id),
            DetailSection::Related | DetailSection::Attachments | DetailSection::Notes => None,
        }?;
        Some(FocusedRelationship {
            section: *section,
            task_id: link.task_id.clone(),
            display_ref: link.display_ref.clone(),
            title: link.title.clone(),
        })
    }

    async fn focused_relationship_selection(
        &self,
        relationship: &FocusedRelationship,
    ) -> Result<Option<crate::tui::task_selection::TaskSelection>> {
        let Some(anchor_index) = self.list.selected_task() else {
            return Ok(None);
        };
        let Some(anchor) = self.store.selected_task(Some(anchor_index)) else {
            return Ok(None);
        };
        let Some(target) = self.store.load_task_item(&relationship.task_id).await? else {
            return Ok(None);
        };
        Ok(Some(
            crate::tui::task_selection::TaskSelection::for_detail_target(
                target,
                anchor,
                anchor_index,
            ),
        ))
    }

    async fn begin_unlink_focused_relationship(
        &mut self,
        relationship: &FocusedRelationship,
    ) -> Result<()> {
        let anchor_id = self
            .store
            .selected_task(self.list.selected_task())
            .map(|item| item.task.id.clone());
        let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
        let intent = match relationship.section {
            DetailSection::DependsOn => {
                let Some(selection) = self.resolve_task_selection() else {
                    self.set_info("no selected task to edit");
                    return Ok(());
                };
                ConfirmIntent::UnlinkDependency {
                    selection,
                    depends_on_task_id: relationship.task_id.clone(),
                }
            }
            DetailSection::Blocks => {
                let Some(selection) = self.focused_relationship_selection(relationship).await?
                else {
                    self.set_warning("linked task is unavailable");
                    return Ok(());
                };
                let Some(depends_on_task_id) = self
                    .store
                    .selected_task(self.list.selected_task())
                    .map(|item| item.task.id.clone())
                else {
                    self.set_info("no selected task to edit");
                    return Ok(());
                };
                ConfirmIntent::UnlinkDependency {
                    selection,
                    depends_on_task_id,
                }
            }
            DetailSection::Related => {
                let Some(selection) = self.resolve_task_selection() else {
                    self.set_info("no selected task to edit");
                    return Ok(());
                };
                ConfirmIntent::UnlinkRelated {
                    selection,
                    related_task_id: relationship.task_id.clone(),
                }
            }
            DetailSection::EpicParent => {
                let Some(target) = self
                    .store
                    .resolve_epic_child_target(self.list.selected_task(), None)
                else {
                    self.set_warning("focused epic relationship is unavailable");
                    return Ok(());
                };
                ConfirmIntent::UnlinkEpicChild {
                    target,
                    restoration: crate::tui::overlay::EpicChildRemovalRestoration {
                        anchor_id: anchor_id
                            .clone()
                            .expect("focused relationship has a selected task"),
                        section: relationship.section,
                        scroll,
                    },
                }
            }
            DetailSection::EpicChildren => {
                let Some(target) = self.store.resolve_epic_child_target(
                    self.list.selected_task(),
                    Some(&relationship.task_id),
                ) else {
                    self.set_warning("focused epic relationship is unavailable");
                    return Ok(());
                };
                ConfirmIntent::UnlinkEpicChild {
                    target,
                    restoration: crate::tui::overlay::EpicChildRemovalRestoration {
                        anchor_id: anchor_id
                            .clone()
                            .expect("focused relationship has a selected task"),
                        section: relationship.section,
                        scroll,
                    },
                }
            }
            DetailSection::Attachments | DetailSection::Notes => {
                self.set_warning("focused row does not support unlink");
                return Ok(());
            }
        };
        self.overlay = Some(OverlayState::confirm(
            intent,
            "Unlink relationship",
            format!(
                "Unlink {} {} from this task?",
                relationship.display_ref, relationship.title
            ),
        ));
        Ok(())
    }

    pub(super) async fn submit_unlink_epic_child(
        &mut self,
        target: crate::tui::store::EpicChildTarget,
        restoration: crate::tui::overlay::EpicChildRemovalRestoration,
    ) -> Result<()> {
        let mut mutation = self.store.remove_epic_child(target).await?;
        if restoration.section == DetailSection::EpicParent {
            mutation.message.selected = self.store.refresh(Some(&restoration.anchor_id)).await?;
        }
        self.list.select_task(mutation.message.selected);
        if mutation.changed
            && restoration.section == DetailSection::EpicChildren
            && let Some(detail) = self.detail.state_mut()
        {
            detail.set_scroll(restoration.scroll);
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
        self.set_mutation_success(mutation.message.message);
        Ok(())
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
        let relationship = self.focused_relationship();
        let domain = target.routing_domain();
        let mut parent_fallback = self.pending_shortcut.clone();
        let parent_fallback_action = match parent_fallback.resolve_detail_in_domain(
            key,
            &self.command_catalog,
            crate::tui::event::RoutingDomain::DetailParent,
        ) {
            DetailShortcutResolution::Action(action) => Some(action),
            _ => None,
        };
        let shortcut = match target {
            DetailTargetId::Task { section, .. } => {
                self.pending_shortcut
                    .resolve_detail_in_focus(key, &self.command_catalog, section)
            }
            _ => self
                .pending_shortcut
                .resolve_detail_in_domain(key, &self.command_catalog, domain),
        };
        match shortcut {
            DetailShortcutResolution::Action(Action::GoBack) => {
                self.pending_shortcut_scroll = 0;
                self.navigate_back_from_detail().await?;
                Ok(true)
            }
            DetailShortcutResolution::Action(Action::GoForward) => {
                self.pending_shortcut_scroll = 0;
                self.navigate_forward_from_detail().await?;
                Ok(true)
            }
            DetailShortcutResolution::Action(Action::BeginStatusPicker)
                if relationship.is_some() =>
            {
                self.pending_shortcut_scroll = 0;
                let Some(selection) = self
                    .focused_relationship_selection(relationship.as_ref().unwrap())
                    .await?
                else {
                    self.set_warning("linked task is unavailable");
                    return Ok(true);
                };
                self.footer_choice = Some(crate::tui::app::FooterChoiceState {
                    mode: FooterChoiceMode::Status,
                    selection,
                });
                Ok(true)
            }
            DetailShortcutResolution::Action(Action::SetStatus(status))
                if relationship.is_some() =>
            {
                self.pending_shortcut_scroll = 0;
                let Some(selection) = self
                    .focused_relationship_selection(relationship.as_ref().unwrap())
                    .await?
                else {
                    self.set_warning("linked task is unavailable");
                    return Ok(true);
                };
                self.submit_edit_status(selection, status.to_string())
                    .await?;
                Ok(true)
            }
            DetailShortcutResolution::Action(Action::CopyShortRef) if relationship.is_some() => {
                self.pending_shortcut_scroll = 0;
                let relationship = relationship.as_ref().unwrap();
                match copy_to_clipboard(&relationship.display_ref) {
                    Ok(()) => self.set_success(format!("copied {}", relationship.display_ref)),
                    Err(error) => self.set_error(format!("copy failed: {error}")),
                }
                Ok(true)
            }
            DetailShortcutResolution::Action(Action::CopyDurableRef) if relationship.is_some() => {
                self.pending_shortcut_scroll = 0;
                let relationship = relationship.as_ref().unwrap();
                match copy_to_clipboard(relationship.task_id.as_str()) {
                    Ok(()) => self.set_success(format!("copied {}", relationship.display_ref)),
                    Err(error) => self.set_error(format!("copy failed: {error}")),
                }
                Ok(true)
            }
            DetailShortcutResolution::Action(Action::CopyTaskTitle) if relationship.is_some() => {
                self.pending_shortcut_scroll = 0;
                let relationship = relationship.as_ref().unwrap();
                match copy_to_clipboard(&relationship.title) {
                    Ok(()) => self.set_success("copied task title"),
                    Err(error) => self.set_error(format!("copy failed: {error}")),
                }
                Ok(true)
            }
            DetailShortcutResolution::Action(Action::Delete) if relationship.is_some() => {
                self.pending_shortcut_scroll = 0;
                let relationship = relationship.as_ref().unwrap();
                let Some(selection) = self.focused_relationship_selection(relationship).await?
                else {
                    self.set_warning("linked task is unavailable");
                    return Ok(true);
                };
                self.overlay = Some(OverlayState::confirm(
                    ConfirmIntent::DeleteFocusedTask { selection },
                    "Delete task",
                    format!(
                        "Delete {} {}?",
                        relationship.display_ref, relationship.title
                    ),
                ));
                Ok(true)
            }
            DetailShortcutResolution::Action(
                action @ (Action::BeginRemoveDependency
                | Action::BeginRemoveRelated
                | Action::RemoveEpicChild),
            ) if relationship.is_some() => {
                self.pending_shortcut_scroll = 0;
                let relationship = relationship.as_ref().unwrap();
                let valid_pair = matches!(
                    (action, relationship.section),
                    (
                        Action::BeginRemoveDependency,
                        DetailSection::DependsOn | DetailSection::Blocks
                    ) | (Action::BeginRemoveRelated, DetailSection::Related)
                        | (
                            Action::RemoveEpicChild,
                            DetailSection::EpicParent | DetailSection::EpicChildren
                        )
                );
                if valid_pair {
                    self.begin_unlink_focused_relationship(relationship).await?;
                } else {
                    self.set_warning("this command does not apply to the focused relationship");
                    self.show_detail(scroll);
                }
                Ok(true)
            }
            DetailShortcutResolution::Action(action) => {
                self.execute_focused_detail_action(action, &target, scroll)
                    .await?;
                Ok(true)
            }
            DetailShortcutResolution::Custom(command_id) => {
                self.pending_shortcut_scroll = 0;
                self.execute_command_handler(CommandHandler::Custom(command_id))
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
            DetailShortcutResolution::PassThrough if relationship.is_some() => {
                self.set_warning("focused relationship does not support that key");
                self.show_detail(scroll);
                Ok(true)
            }
            DetailShortcutResolution::PassThrough if parent_fallback_action.is_some() => {
                self.set_warning(self.detail_focus_warning());
                self.show_detail(scroll);
                Ok(true)
            }
            DetailShortcutResolution::PassThrough => Ok(false),
        }
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
            if matches!(
                *section,
                DetailSection::EpicParent | DetailSection::EpicChildren
            ) {
                self.execute(action).await?;
            } else {
                self.set_warning("this relationship cannot be removed with that command");
                self.show_detail(scroll);
            }
            return Ok(());
        }

        let policy = Some(crate::tui::event::detail_focus_for_action(action));
        let supports_related = matches!(
            (policy, section),
            (Some(DetailFocusPolicy::RelatedTask), _)
                | (
                    Some(DetailFocusPolicy::EpicChild),
                    DetailSection::EpicChildren
                )
        );
        if !supports_related {
            self.set_warning("open the related task before using that command");
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
        let selection = crate::tui::task_selection::TaskSelection::for_detail_target(
            item,
            &anchor,
            anchor_index,
        );
        self.execute_tasks_command(action, selection).await
    }

    async fn handle_detail_shortcut(
        &mut self,
        key: KeyEvent,
        scroll: u16,
    ) -> Result<Option<Option<OverlayState>>> {
        if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
            return Ok(None);
        }

        match self.pending_shortcut.resolve_detail_in_domain(
            key,
            &self.command_catalog,
            crate::tui::event::RoutingDomain::DetailParent,
        ) {
            DetailShortcutResolution::Action(Action::GoBack) => {
                self.pending_shortcut_scroll = 0;
                self.navigate_back_from_detail().await?;
                Ok(Some(self.overlay.take()))
            }
            DetailShortcutResolution::Action(Action::GoForward) => {
                self.pending_shortcut_scroll = 0;
                self.navigate_forward_from_detail().await?;
                Ok(Some(self.overlay.take()))
            }
            DetailShortcutResolution::Action(action) => {
                self.pending_shortcut_scroll = 0;
                if self.store.view_state.query == TaskQuery::Recurring
                    && action == Action::BeginStatusPicker
                {
                    self.set_info(
                        "Status applies to occurrence tasks. Press Enter to open the current occurrence",
                    );
                    if let Some(detail) = self.detail.state_mut() {
                        detail.set_scroll(scroll);
                    }
                    self.show_detail(scroll);
                    return Ok(Some(self.overlay.take()));
                }
                let focus_allows = self
                    .detail
                    .state()
                    .and_then(|detail| detail.focused_target())
                    .is_none_or(|target| {
                        let policy = crate::tui::event::detail_focus_for_action(action);
                        let domain = target.routing_domain();
                        let section =
                            matches!(target, DetailTargetId::Task { .. }).then(|| target.section());
                        crate::tui::event::focus_policy_compatible(policy, domain, section)
                    });
                if !focus_allows {
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
            DetailShortcutResolution::Custom(command_id) => {
                self.pending_shortcut_scroll = 0;
                if let Some(detail) = self.detail.state_mut() {
                    detail.set_scroll(scroll);
                }
                self.execute_command_handler(CommandHandler::Custom(command_id))
                    .await?;
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

    async fn captured_task_selection(
        &self,
        snapshot: &crate::tui::event::CommandSessionSnapshot,
        action: Action,
    ) -> Result<std::result::Result<Option<crate::tui::task_selection::TaskSelection>, String>>
    {
        use crate::tui::event::CommandTargetPolicy;

        let focused_detail_task = snapshot.detail_focus().and_then(|focus| match focus {
            crate::tui::event::DetailCommandFocus::Relationship { task_id, .. } => Some(task_id),
            _ => None,
        });
        let list_marks = matches!(
            snapshot.surface,
            crate::tui::event::CommandSurfaceSnapshot::List { .. }
        )
        .then(|| snapshot.marked_task_ids())
        .unwrap_or(&[]);
        if let CommandTargetPolicy::Single(label) = action.target_policy()
            && list_marks.len() > 1
        {
            return Ok(Err(format!(
                "{label} requires one task · {} tasks marked",
                list_marks.len()
            )));
        }
        let target_ids = match action.target_policy() {
            CommandTargetPolicy::None
            | CommandTargetPolicy::Attachment
            | CommandTargetPolicy::Recurrence => return Ok(Ok(None)),
            CommandTargetPolicy::Marks => match action {
                Action::ToggleMarkSelected => {
                    snapshot.primary_task_id().into_iter().cloned().collect()
                }
                Action::ToggleMarkAllInView => snapshot.visible_task_ids().to_vec(),
                _ => unreachable!("marks policy action uses an ID-only target"),
            },
            CommandTargetPolicy::Single(_) if !list_marks.is_empty() => list_marks.to_vec(),
            CommandTargetPolicy::Focused
            | CommandTargetPolicy::Single(_)
            | CommandTargetPolicy::Relationship(_) => focused_detail_task
                .or_else(|| snapshot.primary_task_id())
                .into_iter()
                .cloned()
                .collect(),
            CommandTargetPolicy::Batch
                if matches!(
                    snapshot.surface,
                    crate::tui::event::CommandSurfaceSnapshot::Detail { .. }
                ) =>
            {
                focused_detail_task
                    .or_else(|| snapshot.primary_task_id())
                    .into_iter()
                    .cloned()
                    .collect()
            }
            CommandTargetPolicy::Batch if !snapshot.marked_task_ids().is_empty() => {
                snapshot.marked_task_ids().to_vec()
            }
            CommandTargetPolicy::Batch => snapshot.primary_task_id().into_iter().cloned().collect(),
        };
        if target_ids.is_empty() {
            let reason = match action {
                Action::BeginAddNote => "no selected task for note",
                Action::AcceptConflictLocal
                | Action::AcceptConflictRemote
                | Action::BeginManualConflictMerge => "no selected task for conflict resolution",
                _ => "no selected task to edit",
            };
            return Ok(Err(reason.to_string()));
        }
        let anchor_id = snapshot.primary_task_id().unwrap_or(&target_ids[0]).clone();
        let mut hydrate_ids = vec![anchor_id.clone()];
        hydrate_ids.extend(target_ids.iter().filter(|id| **id != anchor_id).cloned());
        let hydrated = self.store.load_task_items(&hydrate_ids).await?;
        if hydrated.len() != hydrate_ids.len() {
            return Ok(Err("a captured task is stale".to_string()));
        }
        let anchor = hydrated
            .iter()
            .find(|item| item.task.id == anchor_id)
            .expect("captured anchor was hydrated");
        let targets = target_ids
            .iter()
            .map(|task_id| {
                hydrated
                    .iter()
                    .find(|item| item.task.id == *task_id)
                    .cloned()
                    .expect("captured target was hydrated")
            })
            .collect();
        let anchor_index = self
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == anchor_id)
            .unwrap_or(0);
        let uses_marks = match action.target_policy() {
            CommandTargetPolicy::Batch | CommandTargetPolicy::Single(_) => !list_marks.is_empty(),
            _ => false,
        };
        Ok(Ok(
            crate::tui::task_selection::TaskSelection::from_captured_with_marks(
                targets,
                anchor,
                anchor_index,
                uses_marks,
            ),
        ))
    }

    async fn begin_unlink_captured_relationship(
        &mut self,
        snapshot: &crate::tui::event::CommandSessionSnapshot,
        section: DetailSection,
        related_task_id: &crate::ids::TaskId,
    ) -> Result<()> {
        let Some(parent_id) = snapshot.primary_task_id() else {
            self.set_warning("captured parent task is unavailable");
            return Ok(());
        };
        let items = self
            .store
            .load_task_items(&[parent_id.clone(), related_task_id.clone()])
            .await?;
        let Some(parent) = items.iter().find(|item| item.task.id == *parent_id) else {
            self.set_warning("captured parent task is stale");
            return Ok(());
        };
        if section == DetailSection::Related {
            let Some(link) = parent.related.iter().find(|link| {
                (!link.deleted || parent.task.deleted) && link.task_id == *related_task_id
            }) else {
                self.set_warning("captured relationship is stale");
                return Ok(());
            };
            let selection = crate::tui::task_selection::TaskSelection::from_captured(
                vec![parent.clone()],
                parent,
                0,
            )
            .expect("captured parent selection is non-empty");
            self.overlay = Some(OverlayState::confirm(
                ConfirmIntent::UnlinkRelated {
                    selection,
                    related_task_id: related_task_id.clone(),
                },
                "Unlink relationship",
                format!("Unlink {} {} from this task?", link.display_ref, link.title),
            ));
            return Ok(());
        }
        let link = match section {
            DetailSection::EpicParent => parent
                .epic_parent
                .as_ref()
                .filter(|link| link.task_id == *related_task_id),
            DetailSection::EpicChildren => parent
                .epic_children
                .iter()
                .find(|link| link.task_id == *related_task_id),
            DetailSection::DependsOn => parent
                .depends_on
                .iter()
                .find(|link| link.task_id == *related_task_id),
            DetailSection::Blocks => parent
                .blocks
                .iter()
                .find(|link| link.task_id == *related_task_id),
            DetailSection::Related | DetailSection::Attachments | DetailSection::Notes => None,
        };
        let Some(link) = link else {
            self.set_warning("captured relationship is stale");
            return Ok(());
        };
        let intent = match section {
            DetailSection::DependsOn => {
                let selection = crate::tui::task_selection::TaskSelection::from_captured(
                    vec![parent.clone()],
                    parent,
                    0,
                )
                .expect("captured parent selection is non-empty");
                ConfirmIntent::UnlinkDependency {
                    selection,
                    depends_on_task_id: related_task_id.clone(),
                }
            }
            DetailSection::Blocks => {
                let Some(blocked) = items.iter().find(|item| item.task.id == *related_task_id)
                else {
                    self.set_warning("captured linked task is stale");
                    return Ok(());
                };
                let selection = crate::tui::task_selection::TaskSelection::from_captured(
                    vec![blocked.clone()],
                    parent,
                    0,
                )
                .expect("captured linked selection is non-empty");
                ConfirmIntent::UnlinkDependency {
                    selection,
                    depends_on_task_id: parent_id.clone(),
                }
            }
            DetailSection::EpicParent => {
                let Some(epic) = items.iter().find(|item| item.task.id == *related_task_id) else {
                    self.set_warning("captured epic task is stale");
                    return Ok(());
                };
                let Some((original_position, child)) = epic
                    .epic_children
                    .iter()
                    .enumerate()
                    .find(|(_, child)| child.task_id == *parent_id)
                else {
                    self.set_warning("captured epic relationship is stale");
                    return Ok(());
                };
                ConfirmIntent::UnlinkEpicChild {
                    target: crate::tui::store::EpicChildTarget {
                        epic: crate::tui::store::EpicContext {
                            epic_id: epic.task.id.clone(),
                            display_ref: epic.display_ref.clone(),
                            project_key: epic.task.project_key.clone(),
                        },
                        child: child.clone(),
                        original_position,
                    },
                    restoration: crate::tui::overlay::EpicChildRemovalRestoration {
                        anchor_id: parent_id.clone(),
                        section,
                        scroll: snapshot.detail_scroll().unwrap_or(0),
                    },
                }
            }
            DetailSection::EpicChildren => {
                let original_position = parent
                    .epic_children
                    .iter()
                    .position(|child| child.task_id == *related_task_id)
                    .expect("captured child link has a position");
                ConfirmIntent::UnlinkEpicChild {
                    target: crate::tui::store::EpicChildTarget {
                        epic: crate::tui::store::EpicContext {
                            epic_id: parent.task.id.clone(),
                            display_ref: parent.display_ref.clone(),
                            project_key: parent.task.project_key.clone(),
                        },
                        child: link.clone(),
                        original_position,
                    },
                    restoration: crate::tui::overlay::EpicChildRemovalRestoration {
                        anchor_id: parent_id.clone(),
                        section,
                        scroll: snapshot.detail_scroll().unwrap_or(0),
                    },
                }
            }
            DetailSection::Related | DetailSection::Attachments | DetailSection::Notes => {
                self.set_warning("captured row does not support unlink");
                return Ok(());
            }
        };
        self.overlay = Some(OverlayState::confirm(
            intent,
            "Unlink relationship",
            format!("Unlink {} {} from this task?", link.display_ref, link.title),
        ));
        Ok(())
    }

    pub(super) async fn resolve_builtin_command(
        &self,
        snapshot: &crate::tui::event::CommandSessionSnapshot,
        command: &'static crate::tui::event::BuiltInCommand,
    ) -> Result<std::result::Result<crate::tui::event::ResolvedCommand, String>> {
        use crate::tui::event::{
            CommandSituation, CommandTargetPolicy, DetailCommandFocus, RelationshipTargetPolicy,
            ResolvedCommand, ResolvedCommandTarget,
        };
        if snapshot.workspace.id != self.store.active_workspace.id {
            return Ok(Err("captured workspace is no longer active".to_string()));
        }
        let action = command.action;
        if let crate::tui::event::CommandAvailability::Disabled(reason) =
            crate::tui::event::command_availability(
                crate::tui::event::CatalogCommand::BuiltIn(command),
                snapshot,
                &[],
            )
            && !matches!(reason, crate::tui::event::CommandDisabled::Other(_))
        {
            return Ok(Err(reason.message().to_string()));
        }
        let situation = snapshot.situation();
        let target = if action == Action::ToggleDetail
            && let Some(sidebar) = snapshot.sidebar_target().cloned()
        {
            if let crate::tui::event::SidebarCommandTarget::Project(project) = &sidebar
                && !self
                    .store
                    .projects
                    .iter()
                    .any(|candidate| candidate.key == project.as_str())
            {
                return Ok(Err("captured sidebar project is stale".to_string()));
            }
            ResolvedCommandTarget::Sidebar(sidebar)
        } else if let CommandSituation::SidebarProject { project } = situation
            && matches!(
                action,
                Action::BeginScopeProject
                    | Action::BeginRenameProject
                    | Action::BeginDeleteProject
                    | Action::BeginAddProjectPath
                    | Action::BeginRemoveProjectPath
                    | Action::BeginAddTask
            )
        {
            if !self
                .store
                .projects
                .iter()
                .any(|candidate| candidate.key == project)
            {
                return Ok(Err("captured sidebar project is stale".to_string()));
            }
            ResolvedCommandTarget::SidebarProject(project)
        } else {
            match command.target_policy() {
                CommandTargetPolicy::None => ResolvedCommandTarget::None,
                CommandTargetPolicy::Attachment => {
                    let Some(DetailCommandFocus::Attachment {
                        attachment_id,
                        bytes_present,
                    }) = snapshot.detail_focus()
                    else {
                        return Ok(Err("captured attachment is unavailable".to_string()));
                    };
                    if action == Action::SaveAttachment && !bytes_present {
                        return Ok(Err("attachment bytes are unavailable".to_string()));
                    }
                    let Some(owner) = snapshot.primary_task_id() else {
                        return Ok(Err("captured attachment owner is unavailable".to_string()));
                    };
                    let items = self
                        .store
                        .load_task_items(std::slice::from_ref(owner))
                        .await?;
                    let Some(item) = items.first() else {
                        return Ok(Err("captured attachment owner is stale".to_string()));
                    };
                    let Some(attachment) = item.attachments.iter().find(|attachment| {
                        attachment.attachment_id == *attachment_id && !attachment.deleted
                    }) else {
                        return Ok(Err("captured attachment is stale".to_string()));
                    };
                    if action == Action::SaveAttachment
                        && !self.attachment_bytes_are_available(attachment)
                    {
                        return Ok(Err("attachment bytes are unavailable".to_string()));
                    }
                    ResolvedCommandTarget::Attachment {
                        owner: owner.clone(),
                        attachment_id: attachment_id.clone(),
                    }
                }
                CommandTargetPolicy::Recurrence => {
                    let Some(series_id) = snapshot.recurrence_series_id.clone() else {
                        return Ok(Err("captured recurring series is unavailable".to_string()));
                    };
                    ResolvedCommandTarget::Recurrence(series_id)
                }
                CommandTargetPolicy::Relationship(policy) => {
                    let relationship = match (policy, snapshot.detail_focus()) {
                        (
                            RelationshipTargetPolicy::Dependency,
                            Some(DetailCommandFocus::Relationship {
                                section:
                                    section @ (DetailSection::DependsOn | DetailSection::Blocks),
                                task_id,
                            }),
                        ) => Some((*section, task_id.clone())),
                        (
                            RelationshipTargetPolicy::Related,
                            Some(DetailCommandFocus::Relationship {
                                section: DetailSection::Related,
                                task_id,
                            }),
                        ) => Some((DetailSection::Related, task_id.clone())),
                        (
                            RelationshipTargetPolicy::EpicChild,
                            Some(DetailCommandFocus::Relationship {
                                section: DetailSection::EpicParent,
                                task_id,
                            }),
                        ) => Some((DetailSection::EpicParent, task_id.clone())),
                        (
                            RelationshipTargetPolicy::EpicChild,
                            Some(DetailCommandFocus::Relationship {
                                section: DetailSection::EpicChildren,
                                task_id,
                            }),
                        ) => Some((DetailSection::EpicChildren, task_id.clone())),
                        (_, Some(_)) => {
                            return Ok(Err(
                                "command does not apply to the captured relationship".to_string()
                            ));
                        }
                        (_, None) => None,
                    };
                    let Some((section, related)) = relationship else {
                        let selection = match self.captured_task_selection(snapshot, action).await?
                        {
                            Ok(Some(selection)) => selection,
                            Ok(None) => unreachable!("relationship policy requires a target"),
                            Err(reason) => return Ok(Err(reason)),
                        };
                        if policy == RelationshipTargetPolicy::EpicChild
                            && selection.targets()[0].epic_parent.is_none()
                        {
                            return Ok(Err("captured task is not an epic child".to_string()));
                        }
                        return Ok(Ok(ResolvedCommand {
                            action,
                            target: ResolvedCommandTarget::Tasks(selection),
                            effect: command.surface_effect(),
                        }));
                    };
                    let Some(parent) = snapshot.primary_task_id() else {
                        return Ok(Err("captured parent task is unavailable".to_string()));
                    };
                    ResolvedCommandTarget::Relationship {
                        parent: parent.clone(),
                        related,
                        section,
                    }
                }
                CommandTargetPolicy::Marks if action == Action::ClearMarks => {
                    ResolvedCommandTarget::Marks(snapshot.marked_task_ids().to_vec())
                }
                CommandTargetPolicy::Focused
                | CommandTargetPolicy::Single(_)
                | CommandTargetPolicy::Batch
                | CommandTargetPolicy::Marks => {
                    let selection = match self.captured_task_selection(snapshot, action).await? {
                        Ok(selection) => selection,
                        Err(reason) => return Ok(Err(reason)),
                    };
                    match selection {
                        Some(selection) => ResolvedCommandTarget::Tasks(selection),
                        None => unreachable!("task target policy requires a target"),
                    }
                }
            }
        };
        Ok(Ok(ResolvedCommand {
            action,
            target,
            effect: command.surface_effect(),
        }))
    }

    async fn execute_tasks_command(
        &mut self,
        action: Action,
        selection: crate::tui::task_selection::TaskSelection,
    ) -> Result<()> {
        use crate::tui::app::{TaskCopyKind, TaskRefKind};
        match action {
            Action::MoveColumnLeft => self.move_tasks_by_column_for(selection, -1).await?,
            Action::MoveColumnRight => self.move_tasks_by_column_for(selection, 1).await?,
            Action::BeginMoveToColumn => self.open_move_to_column_picker(selection),
            Action::SetStatus(status) => {
                self.submit_edit_status(selection, status.to_string())
                    .await?
            }
            Action::SetPriority(priority) => {
                self.set_exact_priority_for(selection, priority).await?
            }
            Action::CyclePriority(reverse) => self.update_priority_for(selection, reverse).await?,
            Action::CopyShortRef => self.copy_selected_ref_for(&selection, TaskRefKind::Short),
            Action::CopyDurableRef => self.copy_selected_ref_for(&selection, TaskRefKind::Durable),
            Action::CopyTaskTitle => {
                self.copy_selected_task_text_for(&selection, TaskCopyKind::Title)
            }
            Action::CopyTaskDescription => {
                self.copy_selected_task_text_for(&selection, TaskCopyKind::Description)
            }
            Action::CopyTaskText => {
                self.copy_selected_task_text_for(&selection, TaskCopyKind::TitleAndDescription)
            }
            Action::CopyTaskNotes => self.copy_selected_task_notes_for(&selection),
            Action::CopyTaskMarkdown => {
                self.copy_task_markdown_for(&selection.targets()[0].task.id)
                    .await?
            }
            Action::BeginCreateTaskGist => {
                self.begin_create_task_gist_for(selection.targets()[0].task.id.clone())
            }
            Action::BeginEditTitle => self.begin_edit_title_for(selection),
            Action::BeginEditDescription => self.begin_edit_description_for(selection),
            Action::BeginEditProject => self.open_edit_project_picker(selection),
            Action::BeginEditPriority => self.begin_edit_priority_for(selection),
            Action::BeginEditEpic => self.open_edit_epic_picker(selection),
            Action::BeginEditAvailability => self.begin_edit_availability_for(selection),
            Action::BeginEditDue => self.begin_edit_due_for(selection),
            Action::BeginEditLabels => self.begin_edit_labels_for(selection),
            Action::Delete => self.begin_delete_task_for(selection),
            Action::Restore => {
                let preserve = self.detail.is_active();
                let result = self
                    .store
                    .mutate_deleted_selection(&selection, false, preserve)
                    .await?;
                self.apply_mutation_result(result);
            }
            Action::BeginStatusPicker => self.begin_status_picker_for(selection),
            Action::BeginAddNote => self.begin_add_note_for(selection),
            Action::BeginAddDependency => self.begin_add_dependency_for(selection).await?,
            Action::BeginRemoveDependency => self.open_remove_dependency_picker(selection),
            Action::BeginAddRelated => self.begin_add_related_for(selection).await?,
            Action::BeginRemoveRelated => self.open_remove_related_picker(selection),
            Action::ToggleMarkSelected => {
                self.list
                    .toggle_mark(selection.targets()[0].task.id.clone());
            }
            Action::ToggleMarkAllInView => {
                let ids = selection.ids().cloned().collect::<Vec<_>>();
                if self.list.all_marked(ids.iter()) {
                    for task_id in &ids {
                        self.list.unmark(task_id);
                    }
                } else {
                    self.list.mark_all(ids);
                }
            }
            Action::ClearMarks => unreachable!("clear marks uses an ID-only target"),
            Action::ShowConflictDetails => {
                self.show_conflict_details_for(&selection.targets()[0])
                    .await?
            }
            Action::AcceptConflictLocal
            | Action::AcceptConflictRemote
            | Action::BeginManualConflictMerge => {
                let targets = self
                    .store
                    .conflict_targets_for(&selection.targets()[0])
                    .await?;
                match action {
                    Action::AcceptConflictLocal => self.begin_conflict_resolution_for(
                        crate::tui::conflict_flow::ConflictResolutionChoice::Local,
                        targets,
                    ),
                    Action::AcceptConflictRemote => self.begin_conflict_resolution_for(
                        crate::tui::conflict_flow::ConflictResolutionChoice::Remote,
                        targets,
                    ),
                    Action::BeginManualConflictMerge => {
                        self.begin_manual_conflict_merge_for(targets)
                    }
                    _ => unreachable!("guarded conflict action"),
                }
            }
            Action::ToggleDetail => {
                let item = selection.targets()[0].clone();
                if let Some(index) = self
                    .store
                    .tasks
                    .iter()
                    .position(|candidate| candidate.task.id == item.task.id)
                {
                    self.list.select_task(Some(index));
                } else {
                    self.store.show_exact_task(item);
                    self.list.select_task(Some(0));
                }
                self.show_detail(0);
            }
            Action::RemoveEpicChild => {
                let item = &selection.targets()[0];
                let Some(target) = self.store.resolve_epic_child_target_for_item(item) else {
                    self.set_warning("Selected task does not belong to an epic");
                    return Ok(());
                };
                let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
                self.overlay = Some(OverlayState::confirm(
                    ConfirmIntent::UnlinkEpicChild {
                        target,
                        restoration: crate::tui::overlay::EpicChildRemovalRestoration {
                            anchor_id: item.task.id.clone(),
                            section: DetailSection::EpicParent,
                            scroll,
                        },
                    },
                    "Unlink relationship",
                    format!(
                        "Unlink {} {} from this task?",
                        item.display_ref, item.task.title
                    ),
                ));
            }
            Action::BeginAddEpicChild => {
                let item = &selection.targets()[0];
                let context = if item.task.is_epic {
                    crate::tui::store::AddEpicChildContext::Existing(
                        crate::tui::store::EpicContext {
                            epic_id: item.task.id.clone(),
                            display_ref: item.display_ref.clone(),
                            project_key: item.task.project_key.clone(),
                        },
                    )
                } else if let Some(parent) = &item.epic_parent {
                    crate::tui::store::AddEpicChildContext::Existing(
                        crate::tui::store::EpicContext {
                            epic_id: parent.task_id.clone(),
                            display_ref: parent.display_ref.clone(),
                            project_key: item.task.project_key.clone(),
                        },
                    )
                } else {
                    crate::tui::store::AddEpicChildContext::Promote(
                        crate::tui::store::EpicContext {
                            epic_id: item.task.id.clone(),
                            display_ref: item.display_ref.clone(),
                            project_key: item.task.project_key.clone(),
                        },
                    )
                };
                match context {
                    crate::tui::store::AddEpicChildContext::Existing(epic) => {
                        self.open_add_epic_child_search(epic)
                    }
                    crate::tui::store::AddEpicChildContext::Promote(epic) => {
                        self.clear_live_search_preview();
                        self.overlay = Some(OverlayState::confirm(
                            ConfirmIntent::PromoteTaskForChild { epic: epic.clone() },
                            "Promote task to epic",
                            format!(
                                "Adding a child will promote {} to an epic. Continue?",
                                epic.display_ref
                            ),
                        ));
                    }
                }
            }
            Action::ToggleEpicExpanded => {
                let index = self
                    .store
                    .tasks
                    .iter()
                    .position(|item| item.task.id == selection.targets()[0].task.id);
                if let Some(result) = self.store.toggle_selected_epic(index).await? {
                    self.list.select_task(result.selected);
                } else {
                    self.set_warning("Select an epic in the Epics list");
                }
            }
            _ => self.set_warning("captured command target is unsupported"),
        }
        Ok(())
    }

    pub(super) async fn execute_resolved_builtin(
        &mut self,
        command: crate::tui::event::ResolvedCommand,
        snapshot: &crate::tui::event::CommandSessionSnapshot,
    ) -> Result<()> {
        use crate::tui::event::{ResolvedCommandTarget, SurfaceEffect};
        if command.effect == SurfaceEffect::ExitDetail {
            self.clear_detail_session();
        }
        match command.target {
            ResolvedCommandTarget::None => self.execute(command.action).await?,
            ResolvedCommandTarget::Marks(task_ids) => {
                for task_id in task_ids {
                    self.list.unmark(&task_id);
                }
            }
            ResolvedCommandTarget::Tasks(selection) => {
                self.execute_tasks_command(command.action, selection)
                    .await?
            }
            ResolvedCommandTarget::Relationship {
                parent,
                related,
                section,
            } => {
                debug_assert_eq!(snapshot.primary_task_id(), Some(&parent));
                let valid_pair = matches!(
                    (command.action, section),
                    (
                        Action::BeginRemoveDependency,
                        DetailSection::DependsOn | DetailSection::Blocks
                    ) | (Action::BeginRemoveRelated, DetailSection::Related)
                        | (
                            Action::RemoveEpicChild,
                            DetailSection::EpicParent | DetailSection::EpicChildren
                        )
                );
                if !valid_pair {
                    self.set_warning("captured relationship command is invalid");
                    return Ok(());
                }
                if let (Some(scroll), Some(detail)) =
                    (snapshot.detail_scroll(), self.detail.state_mut())
                {
                    detail.set_scroll(scroll);
                }
                self.begin_unlink_captured_relationship(snapshot, section, &related)
                    .await?
            }
            ResolvedCommandTarget::Sidebar(sidebar) => {
                if command.action != Action::ToggleDetail {
                    self.set_warning("captured sidebar command is invalid");
                    return Ok(());
                }
                self.apply_sidebar_command_target(sidebar).await?;
            }
            ResolvedCommandTarget::SidebarProject(project) => match command.action {
                Action::BeginScopeProject => {
                    self.show_scope(crate::tui::store::TaskScopeTarget::Project(project))
                        .await?
                }
                Action::BeginRenameProject => self.begin_rename_project_for(Some(&project)),
                Action::BeginDeleteProject => self.begin_delete_project_for(Some(&project)),
                Action::BeginAddProjectPath => self.begin_add_project_path_for(Some(&project)),
                Action::BeginRemoveProjectPath => {
                    self.begin_remove_project_path_for(Some(&project))
                }
                Action::BeginAddTask => self.begin_add_task_for(Some(project)).await?,
                _ => self.execute(command.action).await?,
            },
            ResolvedCommandTarget::Attachment {
                owner,
                attachment_id,
            } => {
                let items = self
                    .store
                    .load_task_items(std::slice::from_ref(&owner))
                    .await?;
                let Some(attachment) = items.first().and_then(|item| {
                    item.attachments.iter().find(|attachment| {
                        attachment.attachment_id == attachment_id && !attachment.deleted
                    })
                }) else {
                    self.set_warning("captured attachment is stale");
                    return Ok(());
                };
                let attachment = attachment.clone();
                let scroll = self.detail.state().map_or(0, |detail| detail.scroll());
                match command.action {
                    Action::OpenAttachment => self.open_attachment_externally(&attachment_id).await,
                    Action::SaveAttachment => {
                        self.begin_save_attachment_metadata(&attachment, scroll)
                    }
                    Action::DeleteAttachment => {
                        self.begin_delete_attachment_metadata(&attachment, scroll)
                    }
                    _ => unreachable!("attachment target policy"),
                }
            }
            ResolvedCommandTarget::Recurrence(series_id) => {
                self.execute_targeted_recurrence_action(
                    Some(crate::tui::overlay::OverlayTarget::RecurrenceSeries {
                        workspace_id: snapshot.workspace.id.clone(),
                        series_id,
                    }),
                    command.action,
                )
                .await?
            }
        }
        Ok(())
    }

    async fn accept_command_input(&mut self, state: &CommandState) -> Result<bool> {
        let input = state.input.as_str();
        if input.trim().trim_start_matches(':').is_empty() {
            self.set_info("empty command");
            return Ok(false);
        }
        let candidate = if let Some(highlighted) = state.highlighted {
            state.candidates.get(highlighted)
        } else {
            let normalized = input.trim().trim_start_matches(':');
            let mut ranked = state.candidates.iter().filter_map(|candidate| {
                state.catalog.command(candidate.index).and_then(|command| {
                    crate::tui::event::command_match_rank_for_query(command, normalized)
                        .map(|rank| (rank, candidate))
                })
            });
            let Some((best_rank, first)) = ranked.next() else {
                self.set_warning(format!("unknown command: {}", input.trim()));
                return Ok(false);
            };
            if ranked.any(|(rank, _)| rank == best_rank) {
                self.set_warning(format!("ambiguous command: {}", input.trim()));
                return Ok(false);
            }
            Some(first)
        };
        let Some(candidate) = candidate else {
            self.set_warning(format!("unknown command: {}", input.trim()));
            return Ok(false);
        };
        let Some(command) = state.catalog.command(candidate.index) else {
            self.set_warning("command disappeared from the captured catalog");
            return Ok(true);
        };
        if let Some(reason) = candidate.availability.reason() {
            self.set_warning(format!(":{} is disabled: {reason}", command.name()));
            return Ok(true);
        }
        self.pending_shortcut.clear();
        match command.handler() {
            CommandHandler::Custom(id) => {
                let catalog = state.catalog.clone();
                self.execute_captured_custom_command(
                    &catalog,
                    id,
                    input.trim().trim_start_matches(':'),
                    &state.session,
                )
                .await?;
            }
            CommandHandler::BuiltIn(_) => {
                let built_in = command.built_in().expect("built-in command");
                let resolved = match self
                    .resolve_builtin_command(&state.session, built_in)
                    .await?
                {
                    Ok(resolved) => resolved,
                    Err(reason) => {
                        self.set_warning(format!(":{} is disabled: {reason}", built_in.name));
                        return Ok(true);
                    }
                };
                self.execute_resolved_builtin(resolved, &state.session)
                    .await?;
            }
        }
        Ok(true)
    }

    fn move_command_selection(&self, state: &mut CommandState, reverse: bool) {
        if state.candidates.is_empty() {
            state.highlighted = None;
            return;
        }
        state.highlighted = Some(match (state.highlighted, reverse) {
            (Some(0), true) | (None, true) => state.candidates.len() - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) if index + 1 == state.candidates.len() => 0,
            (Some(index), false) => index + 1,
            (None, false) => 0,
        });
    }

    fn complete_command_input(&mut self, state: &mut CommandState, reverse: bool) {
        if state.cycle_input.is_none() {
            state.cycle_input = Some(state.input.text.clone());
            state.cycle_candidates = state
                .candidates
                .iter()
                .map(|candidate| candidate.index)
                .collect();
            state.cycle_index = if reverse {
                state.cycle_candidates.len().saturating_sub(1)
            } else {
                0
            };
        } else if !state.cycle_candidates.is_empty() {
            state.cycle_index = if reverse {
                state
                    .cycle_index
                    .checked_sub(1)
                    .unwrap_or(state.cycle_candidates.len() - 1)
            } else {
                (state.cycle_index + 1) % state.cycle_candidates.len()
            };
        }
        let Some(index) = state.cycle_candidates.get(state.cycle_index).copied() else {
            if state
                .input
                .as_str()
                .trim()
                .trim_start_matches(':')
                .is_empty()
            {
                self.set_info("type a command prefix to complete");
            } else {
                self.set_warning(format!(
                    "no command matches: {}",
                    state.input.as_str().trim()
                ));
            }
            state.reset_cycle();
            return;
        };
        let Some(command) = state.catalog.command(index) else {
            state.reset_cycle();
            return;
        };
        state.input.text = command.name().to_string();
        state.input.cursor = state.input.text.len();
        state.refresh_candidates();
        state.highlighted = state
            .candidates
            .iter()
            .position(|candidate| candidate.index == index);
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

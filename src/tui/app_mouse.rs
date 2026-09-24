use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};
use std::time::{Duration, Instant};

use crate::tui::app::{App, Focus};
use crate::tui::input::mouse::{
    MouseInput, PointerEvent, TaskSurfaceView, route_mouse, route_task_surface,
};
use crate::tui::navigation::{detail_scroll_with_delta_with_images, next_index, scroll_with_delta};
use crate::tui::overlay::OverlayState;
use crate::tui::store::TaskQuery;
use crate::tui::ui::{prefix_hint_scroll_cap, task_at_position, task_status_at_position};

impl App {
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

    pub(super) async fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
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
                crate::tui::ui::HeaderTarget::SyncStatus => {
                    self.show_sync_dialog();
                    Ok(())
                }
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
                    && let Some(origin_lane) = self
                        .store
                        .column_board()
                        .position(hit.task_index)
                        .map(|(lane, _)| lane)
                {
                    self.list.begin_column_drag(
                        hit.task_id,
                        origin_lane,
                        (mouse.column, mouse.row),
                    );
                }
            }
            PointerEvent::SelectSidebar(target) => {
                self.list.clear_task_click();
                self.list.select_sidebar_target(Some(&target));
                self.apply_sidebar_target(Some(target)).await?;
            }
            PointerEvent::None => self.list.clear_task_click(),
        }

        Ok(())
    }

    pub(super) fn task_area_for_mouse(&self, terminal_size: Size) -> Rect {
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

    pub(super) async fn handle_task_status_right_click(
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

    pub(super) fn sidebar_contains_mouse(
        &self,
        terminal_size: Size,
        column: u16,
        row: u16,
    ) -> bool {
        let terminal = Rect::new(0, 0, terminal_size.width, terminal_size.height);
        crate::tui::ui::sidebar_layout_for(terminal, self.list.focus(), self.list.sidebar_visible())
            .is_some_and(|layout| {
                column >= layout.sidebar.x
                    && column < layout.sidebar.x.saturating_add(layout.sidebar.width)
                    && row >= layout.sidebar.y
                    && row < layout.sidebar.y.saturating_add(layout.sidebar.height)
            })
    }

    pub(super) fn shortcut_overlay_scroll(&self, kind: MouseEventKind) -> Option<u16> {
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

    pub(super) fn routes_wheel_to_task_list(
        &self,
        kind: MouseEventKind,
        terminal_size: Size,
    ) -> bool {
        matches!(kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp)
            && !self.prefix_hints_active()
            && self.overlay.is_none()
            && terminal_size.width >= 70
            && terminal_size.height >= 18
            && !self.detail_underlay()
            && self.list.focus() == Focus::Tasks
    }

    pub(super) async fn handle_task_list_wheel(
        &mut self,
        delta: isize,
        terminal_size: Size,
    ) -> Result<()> {
        if self.overlay.is_some()
            || terminal_size.width < 70
            || terminal_size.height < 18
            || self.detail_underlay()
            || self.list.focus() != Focus::Tasks
        {
            return Ok(());
        }

        let next = if self.store.view_state.is_columns() {
            self.store
                .column_board()
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

    pub(super) fn dispatch_prefix_hint_scroll(
        &mut self,
        delta: isize,
        terminal_size: Size,
    ) -> bool {
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

    pub(super) fn prefix_hints_active(&self) -> bool {
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

    pub(super) fn dispatch_mouse_scroll(
        &mut self,
        kind: MouseEventKind,
        terminal_size: Size,
    ) -> bool {
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
}

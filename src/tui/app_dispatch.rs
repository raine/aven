use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};
use std::time::Instant;

use crate::tui::app::{
    App, DetailNavigationState, Focus, FooterChoiceMode, TASK_ROW_DOUBLE_CLICK, TaskCopyKind,
    TaskRefKind,
};
use crate::tui::authoring::AddTaskStep;
use crate::tui::conflict_flow::ConflictResolutionChoice;
use crate::tui::event::{
    Action, CommandCompletion, CommandLookup, command_cycle_options, complete_command,
    lookup_command,
};
use crate::tui::navigation::{
    detail_scroll_with_delta, detail_task_delta, handle_detail_overlay_key, next_index,
    scroll_with_delta,
};
use crate::tui::overlay::{AddTaskMode, CommandState, OverlayOutcome, OverlayRoute, OverlayState};
use crate::tui::platform::is_editor_prefix_key;
use crate::tui::shortcut_buffer::{DetailShortcutResolution, NormalShortcutResolution};
use crate::tui::store::TaskView;
use crate::tui::ui::{
    database_stats_scroll_cap, detail_child_task_at_position, detail_help_scroll_cap,
    detail_section_scroll_target, detail_selected_text, detail_text_cell_at_position,
    help_scroll_cap, prefix_hint_scroll_cap, recent_action_at_position, task_at_position,
    task_status_at_position, text_panel_scroll_cap,
};

impl App {
    pub(super) async fn dispatch_paste(&mut self, text: &str) -> Result<()> {
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
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.handle(Action::Quit).await
        } else if self.footer_choice_mode.is_some() {
            self.handle_footer_choice_key(key).await
        } else if key.code == KeyCode::Esc && self.pending_shortcut.cancel() {
            self.pending_shortcut_scroll = 0;
            Ok(())
        } else if self.overlay_captures_input() {
            if key.code == KeyCode::Char('?')
                && matches!(self.overlay, Some(OverlayState::Detail { .. }))
            {
                self.toggle_help_at_height(terminal_size.height);
                Ok(())
            } else if self.dispatch_prefix_hint_key_scroll(key, terminal_size) {
                Ok(())
            } else {
                self.handle_overlay_key_at_size(key, terminal_size).await
            }
        } else if key.code == KeyCode::Char('?') {
            self.toggle_help_at_height(terminal_size.height);
            Ok(())
        } else if self.dispatch_prefix_hint_key_scroll(key, terminal_size) {
            Ok(())
        } else if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
            self.handle_normal_key(key.code).await
        } else {
            Ok(())
        }
    }

    async fn handle_footer_choice_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(mode) = self.footer_choice_mode else {
            return Ok(());
        };
        if !key.modifiers.is_empty() {
            return Ok(());
        }
        match (mode, key.code) {
            (_, KeyCode::Esc) => {
                self.footer_choice_mode = None;
                Ok(())
            }
            (FooterChoiceMode::Status, KeyCode::Char('i')) => {
                self.submit_footer_status("inbox").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('b')) => {
                self.submit_footer_status("backlog").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('t')) => {
                self.submit_footer_status("todo").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('a')) => {
                self.submit_footer_status("active").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('d')) => {
                self.submit_footer_status("done").await
            }
            (FooterChoiceMode::Status, KeyCode::Char('x')) => {
                self.submit_footer_status("canceled").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('n')) => {
                self.submit_footer_priority("none").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('l')) => {
                self.submit_footer_priority("low").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('m')) => {
                self.submit_footer_priority("medium").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('h')) => {
                self.submit_footer_priority("high").await
            }
            (FooterChoiceMode::Priority, KeyCode::Char('u')) => {
                self.submit_footer_priority("urgent").await
            }
            _ => Ok(()),
        }
    }

    async fn submit_footer_status(&mut self, status: &'static str) -> Result<()> {
        self.footer_choice_mode = None;
        self.update_status(status).await?;
        if self.detail_context && self.overlay.is_none() {
            self.restore_detail_overlay(true);
        }
        Ok(())
    }

    async fn submit_footer_priority(&mut self, priority: &'static str) -> Result<()> {
        self.footer_choice_mode = None;
        self.set_exact_priority(priority).await?;
        if self.detail_context && self.overlay.is_none() {
            self.restore_detail_overlay(true);
        }
        Ok(())
    }

    pub(crate) async fn dispatch_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if self.dispatch_prefix_hint_scroll(1, terminal_size) {
                    return Ok(());
                }
                if self.dispatch_mouse_scroll(mouse.kind, terminal_size) {
                    return Ok(());
                }
                return self.handle_task_list_wheel(1, terminal_size).await;
            }
            MouseEventKind::ScrollUp => {
                if self.dispatch_prefix_hint_scroll(-1, terminal_size) {
                    return Ok(());
                }
                if self.dispatch_mouse_scroll(mouse.kind, terminal_size) {
                    return Ok(());
                }
                return self.handle_task_list_wheel(-1, terminal_size).await;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.begin_detail_text_selection(mouse, terminal_size) {
                    return Ok(());
                }
                self.detail_text_selection = None;
                self.detail_text_dragging = false;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.update_detail_text_selection(mouse, terminal_size);
                return Ok(());
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.detail_text_dragging = false;
                return Ok(());
            }
            MouseEventKind::Moved => {
                self.handle_detail_mouse_move(mouse, terminal_size);
                return Ok(());
            }
            MouseEventKind::Down(MouseButton::Right) => {
                return self
                    .handle_task_status_right_click(mouse, terminal_size)
                    .await;
            }
            _ => return Ok(()),
        }

        self.last_task_click = self
            .last_task_click
            .as_ref()
            .filter(|last| last.at.elapsed() <= TASK_ROW_DOUBLE_CLICK)
            .cloned();

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
            self.last_task_click = None;
            self.cancel_overlay();
            return Ok(());
        }

        if matches!(self.overlay, Some(OverlayState::HeaderMenu(_))) {
            let Some(OverlayState::HeaderMenu(state)) = self.overlay.take() else {
                return Ok(());
            };
            self.submit_header_menu_at(state, mouse.column, mouse.row, terminal_size)
                .await?;
            if self.detail_context && self.overlay.is_none() {
                self.restore_detail_overlay(true);
            }
            return Ok(());
        }
        if matches!(self.overlay, Some(OverlayState::OrderMenu(_))) {
            let Some(OverlayState::OrderMenu(state)) = self.overlay.take() else {
                return Ok(());
            };
            self.submit_order_menu_at(state, mouse.column, mouse.row, terminal_size)
                .await?;
            if self.detail_context && self.overlay.is_none() {
                self.restore_detail_overlay(true);
            }
            return Ok(());
        }
        if let Some(OverlayState::Detail { scroll }) = self.overlay
            && self.handle_detail_mouse_click(mouse, terminal_size, scroll)
        {
            return Ok(());
        }
        let add_task_field = self.overlay.as_ref().and_then(|overlay| match overlay {
            OverlayState::AddTask(state) if state.mode == AddTaskMode::Compose => {
                crate::tui::ui::add_task_field_at(
                    Rect::new(0, 0, terminal_size.width, terminal_size.height),
                    self.add_task_only,
                    mouse.column,
                    mouse.row,
                )
            }
            _ => None,
        });
        if let Some(field) = add_task_field {
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
            self.last_task_click = None;
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
            self.last_task_click = None;
            return match target {
                crate::tui::ui::HeaderTarget::Home => Ok(()),
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

        if !self.sidebar_contains_mouse(terminal_size, mouse.column, mouse.row)
            && self.store.view_state.view == TaskView::Columns
            && let Some(lane_index) = crate::tui::ui::column_lane_at_position(
                &self.store,
                &self.widgets.table,
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
            && let Some(status) =
                crate::tui::columns::lane_entry_status(&self.store.task_columns, lane_index)
        {
            self.last_task_click = None;
            return self.move_tasks_to_column(status.as_str().to_string()).await;
        }

        if !self.sidebar_contains_mouse(terminal_size, mouse.column, mouse.row)
            && self.store.view_state.view == TaskView::RecentActions
            && let Some(hit) = recent_action_at_position(
                &self.store,
                &self.widgets.table,
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
        {
            self.focus = Focus::Tasks;
            self.widgets.table.select(Some(hit.action_index));
            self.last_task_click = None;
            return Ok(());
        }

        if !self.sidebar_contains_mouse(terminal_size, mouse.column, mouse.row)
            && let Some(hit) = task_status_at_position(
                &self.store,
                &self.widgets.table,
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
        {
            self.last_task_click = None;
            self.focus = Focus::Tasks;
            self.widgets.table.select(Some(hit.task_index));
            self.footer_choice_mode = Some(FooterChoiceMode::Status);
            return Ok(());
        }

        if !self.sidebar_contains_mouse(terminal_size, mouse.column, mouse.row)
            && let Some(hit) = task_at_position(
                &self.store,
                &self.widgets.table,
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
        {
            self.focus = Focus::Tasks;
            self.widgets.table.select(Some(hit.task_index));
            let now = Instant::now();
            let is_double_click = self.last_task_click.as_ref().is_some_and(|previous| {
                previous.task_id == hit.task_id
                    && previous.viewport_row == hit.viewport_row
                    && now.duration_since(previous.at) <= TASK_ROW_DOUBLE_CLICK
            });
            if is_double_click {
                self.last_task_click = None;
                self.overlay = Some(OverlayState::Detail { scroll: 0 });
            } else {
                self.last_task_click = Some(crate::tui::app::TaskRowClick {
                    task_id: hit.task_id,
                    viewport_row: hit.viewport_row,
                    at: now,
                });
            }
            return Ok(());
        }

        self.last_task_click = None;
        if let Some(click) = crate::tui::ui::sidebar_click_at_for(
            &self.store.sidebar_entries,
            &self.widgets.sidebar,
            self.focus,
            self.sidebar_visible,
            ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
            mouse.column,
            mouse.row,
        ) {
            self.widgets.sidebar.select(Some(click.entry_index));
            self.apply_sidebar_selection().await?;
        }

        Ok(())
    }

    fn task_area_for_mouse(&self, terminal_size: Size) -> Rect {
        let body_height = terminal_size.height.saturating_sub(4);
        let body = Rect::new(0, 2, terminal_size.width, body_height);
        if !self.sidebar_visible || body.width < 100 {
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
        self.last_task_click = None;
        if self.overlay.is_some() || terminal_size.width < 70 || terminal_size.height < 18 {
            return Ok(());
        }
        if self.sidebar_contains_mouse(terminal_size, mouse.column, mouse.row) {
            return Ok(());
        }
        let hit = if self.store.view_state.view == TaskView::Columns {
            task_at_position(
                &self.store,
                &self.widgets.table,
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
        } else {
            task_status_at_position(
                &self.store,
                &self.widgets.table,
                self.task_area_for_mouse(terminal_size),
                mouse.column,
                mouse.row,
            )
        };
        let Some(hit) = hit else {
            return Ok(());
        };

        self.focus = Focus::Tasks;
        self.widgets.table.select(Some(hit.task_index));
        self.footer_choice_mode = Some(FooterChoiceMode::Status);
        Ok(())
    }

    fn sidebar_contains_mouse(&self, terminal_size: Size, column: u16, row: u16) -> bool {
        let terminal = Rect::new(0, 0, terminal_size.width, terminal_size.height);
        crate::tui::ui::sidebar_layout_for(terminal, self.focus, self.sidebar_visible).is_some_and(
            |layout| {
                column >= layout.sidebar.x
                    && column < layout.sidebar.x.saturating_add(layout.sidebar.width)
                    && row >= layout.sidebar.y
                    && row < layout.sidebar.y.saturating_add(layout.sidebar.height)
            },
        )
    }

    fn begin_detail_text_selection(&mut self, mouse: MouseEvent, terminal_size: Size) -> bool {
        let Some(OverlayState::Detail { scroll }) = self.overlay else {
            return false;
        };
        let Some(item) = self.store.selected_task(self.widgets.table.selected()) else {
            return false;
        };
        let Some(cell) = detail_text_cell_at_position(
            item,
            terminal_size.width,
            terminal_size.height,
            mouse.column,
            mouse.row,
            scroll,
        ) else {
            return false;
        };
        self.detail_text_selection = Some(crate::tui::detail_selection::DetailTextSelection::new(
            item.task.id.clone(),
            terminal_size.width,
            cell,
        ));
        self.detail_text_dragging = true;
        self.last_task_click = None;
        true
    }

    fn update_detail_text_selection(&mut self, mouse: MouseEvent, terminal_size: Size) {
        if !self.detail_text_dragging {
            return;
        }
        let Some(OverlayState::Detail { scroll }) = self.overlay else {
            return;
        };
        let Some(item) = self.store.selected_task(self.widgets.table.selected()) else {
            return;
        };
        let Some(cell) = detail_text_cell_at_position(
            item,
            terminal_size.width,
            terminal_size.height,
            mouse.column,
            mouse.row,
            scroll,
        ) else {
            return;
        };
        if let Some(selection) = self.detail_text_selection.as_mut()
            && selection.task_id == item.task.id
            && selection.terminal_width == terminal_size.width
        {
            selection.focus = cell;
        }
    }

    fn copy_detail_text_selection(&mut self) {
        let value = self.detail_text_selection.as_ref().and_then(|selection| {
            self.store
                .selected_task(self.widgets.table.selected())
                .and_then(|item| detail_selected_text(item, selection))
        });
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            self.set_info("no detail text selected");
            return;
        };
        match crate::tui::platform::copy_to_clipboard(&value) {
            Ok(()) => self.set_success("copied selected text"),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
    }

    fn handle_detail_mouse_click(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
        scroll: u16,
    ) -> bool {
        if let Some(item) = self.store.selected_task(self.widgets.table.selected()) {
            let current_task_id = item.task.id.clone();
            if let Some(hit) = detail_child_task_at_position(
                item,
                terminal_size.width,
                terminal_size.height,
                mouse.column,
                mouse.row,
                scroll,
            ) {
                self.last_task_click = None;
                self.detail_context = true;
                self.detail_context_scroll = 0;
                self.hovered_detail_child_task_id = Some(hit.task_id.clone());
                if let Some(index) = self
                    .store
                    .tasks
                    .iter()
                    .position(|item| item.task.id == hit.task_id)
                {
                    self.push_detail_navigation_state(DetailNavigationState {
                        task_id: current_task_id,
                        scroll,
                    });
                    self.widgets.table.select(Some(index));
                    self.overlay = Some(OverlayState::Detail { scroll: 0 });
                } else {
                    self.set_warning("child task is hidden by the current view");
                    if self.overlay.is_none() {
                        self.overlay = Some(OverlayState::Detail { scroll });
                    }
                }
                return true;
            }
        }

        let Some((target, _column, _row)) = crate::tui::ui::detail_metadata_target_at(
            terminal_size.width,
            terminal_size.height,
            mouse.column,
            mouse.row,
        ) else {
            return false;
        };
        self.last_task_click = None;
        self.detail_context = true;
        self.detail_context_scroll = scroll;
        match target {
            crate::tui::ui::DetailMetadataTarget::Status => {
                self.footer_choice_mode = Some(FooterChoiceMode::Status)
            }
            crate::tui::ui::DetailMetadataTarget::Priority => {
                self.footer_choice_mode = Some(FooterChoiceMode::Priority)
            }
        }
        if self.overlay.is_none() {
            self.overlay = Some(OverlayState::Detail { scroll });
        }
        true
    }

    fn handle_detail_mouse_move(&mut self, mouse: MouseEvent, terminal_size: Size) {
        let Some(OverlayState::Detail { scroll }) = self.overlay else {
            self.hovered_detail_child_task_id = None;
            return;
        };
        self.hovered_detail_child_task_id = self
            .store
            .selected_task(self.widgets.table.selected())
            .and_then(|item| {
                detail_child_task_at_position(
                    item,
                    terminal_size.width,
                    terminal_size.height,
                    mouse.column,
                    mouse.row,
                    scroll,
                )
            })
            .map(|hit| hit.task_id);
    }

    async fn handle_task_list_wheel(&mut self, delta: isize, terminal_size: Size) -> Result<()> {
        if self.overlay.is_some()
            || terminal_size.width < 70
            || terminal_size.height < 18
            || self.detail_underlay()
            || self.focus != Focus::Tasks
        {
            return Ok(());
        }

        let next = if self.store.view_state.view == TaskView::Columns {
            crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                .move_vertical_bounded(self.widgets.table.selected(), delta)
        } else {
            next_index(
                self.widgets.table.selected(),
                self.store.main_row_count(),
                delta,
                false,
            )
        };
        self.widgets.table.select(next);
        Ok(())
    }

    fn dispatch_prefix_hint_key_scroll(&mut self, key: KeyEvent, terminal_size: Size) -> bool {
        if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
            return false;
        }
        let delta = match key.code {
            KeyCode::Down => 1,
            KeyCode::Up => -1,
            KeyCode::PageDown => terminal_size.height.saturating_sub(4).max(1) as isize,
            KeyCode::PageUp => -(terminal_size.height.saturating_sub(4).max(1) as isize),
            _ => return false,
        };
        self.dispatch_prefix_hint_scroll(delta, terminal_size)
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
            Some(OverlayState::Detail { scroll }) => {
                let task = self.store.selected_task(self.widgets.table.selected());
                *scroll = detail_scroll_with_delta(
                    *scroll,
                    delta,
                    terminal_size.width,
                    terminal_size.height,
                    task,
                );
                true
            }
            Some(OverlayState::TextPanel(state)) => {
                let cap = text_panel_scroll_cap(&state.lines);
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
        if self.overlay_captures_input()
            && (code != KeyCode::Esc || self.pending_shortcut.is_empty())
        {
            return self
                .handle_overlay_key(KeyEvent::new(code, KeyModifiers::NONE))
                .await;
        }

        if code == KeyCode::Esc {
            if !self.pending_shortcut.cancel() {
                self.handle(Action::CancelOverlay).await?;
            }
            self.pending_shortcut_scroll = 0;
            return Ok(());
        }

        match self.pending_shortcut.resolve_normal(code) {
            NormalShortcutResolution::Action(action) => {
                self.pending_shortcut_scroll = 0;
                self.handle(action).await?;
            }
            NormalShortcutResolution::Prefix => {
                self.pending_shortcut_scroll = 0;
            }
            NormalShortcutResolution::Missing(label) => {
                self.pending_shortcut_scroll = 0;
                self.set_warning(format!("invalid shortcut: {label}"));
            }
        }
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
        let Some(overlay) = self.overlay.take() else {
            return Ok(());
        };

        match overlay {
            OverlayState::Search(state) => self.handle_search_key(state, key).await?,
            OverlayState::Update(state) => self.handle_update_overlay_key(state, key).await,
            OverlayState::Command { mut state } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(action) = self.accept_command_input(state.input.as_str()) {
                        self.execute(action).await?;
                    } else {
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

        if self.detail_context && self.overlay.is_none() {
            self.restore_detail_overlay(true);
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
                    state.route,
                    OverlayRoute::AddTaskTitleProject | OverlayRoute::AddTaskTitlePriority
                )
        ) || matches!(
            &overlay,
            OverlayState::TagCombobox(state) if state.route == OverlayRoute::AddTaskTitleLabels
        );
        let outcome =
            crate::tui::overlay::handle_generic_overlay_mouse(overlay, mouse, terminal_size);
        self.apply_generic_overlay_outcome(outcome, false, false, was_add_task_picker)
            .await?;
        if self.detail_context && self.overlay.is_none() {
            self.restore_detail_overlay(true);
        }
        Ok(())
    }

    async fn handle_generic_overlay_key(
        &mut self,
        key: KeyEvent,
        overlay: OverlayState,
        terminal_size: Size,
    ) -> Result<()> {
        if let OverlayState::Detail { scroll } = overlay {
            if key.code == KeyCode::Esc && self.detail_text_selection.take().is_some() {
                self.detail_text_dragging = false;
                self.overlay = Some(OverlayState::Detail { scroll });
                return Ok(());
            }
            if key.code == KeyCode::Char('y')
                && key.modifiers.is_empty()
                && self.detail_text_selection.is_some()
            {
                self.pending_shortcut.clear();
                self.pending_shortcut_scroll = 0;
                self.copy_detail_text_selection();
                self.overlay = Some(OverlayState::Detail { scroll });
                return Ok(());
            }
            let section_direction = match (key.code, key.modifiers) {
                (KeyCode::Tab, KeyModifiers::NONE) => Some(false),
                (KeyCode::BackTab, KeyModifiers::NONE | KeyModifiers::SHIFT)
                | (KeyCode::Tab, KeyModifiers::SHIFT) => Some(true),
                _ => None,
            };
            if let (Some(reverse), Some(task)) = (
                section_direction,
                self.store.selected_task(self.widgets.table.selected()),
            ) {
                self.overlay = Some(OverlayState::Detail {
                    scroll: detail_section_scroll_target(
                        task,
                        scroll,
                        terminal_size.width,
                        terminal_size.height,
                        reverse,
                    ),
                });
                return Ok(());
            }

            if let Some(outcome) = self.handle_detail_shortcut(key, scroll).await? {
                self.overlay = outcome;
                return Ok(());
            }

            if let Some(delta) = detail_task_delta(key) {
                self.select_detail_task(delta);
                self.overlay = Some(OverlayState::Detail { scroll: 0 });
                return Ok(());
            }

            let overlay = OverlayState::Detail { scroll };
            let task = self.store.selected_task(self.widgets.table.selected());
            let outcome = handle_detail_overlay_key(
                key,
                overlay,
                terminal_size.width,
                terminal_size.height,
                task,
            );
            match outcome {
                OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
                OverlayOutcome::Cancelled => {
                    self.cancel_authoring_overlay();
                    self.refresh().await?;
                }
                OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
            }
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
                OverlayState::MultilineInput(state)
                    if state.route == OverlayRoute::EditDescription =>
                {
                    self.open_description_external_editor(state.clone());
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
                    if state.route == OverlayRoute::EditDescription
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
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('a') {
                self.overlay = Some(overlay);
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    state.focus = AddTaskStep::AvailableAt;
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('u')
                && state.focus != AddTaskStep::Due
            {
                self.overlay = Some(overlay);
                if let Some(OverlayState::AddTask(state)) = self.overlay.as_mut() {
                    state.focus = AddTaskStep::Due;
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

        let scroll_cap = match overlay {
            OverlayState::DetailHelp { .. } => detail_help_scroll_cap(terminal_size.height),
            OverlayState::DatabaseStats { .. } => database_stats_scroll_cap(terminal_size.height),
            _ => help_scroll_cap(terminal_size.height),
        };
        let was_detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
        let was_add_task_description_editor = matches!(
            &overlay,
            OverlayState::MultilineInput(state) if state.route == OverlayRoute::AddTaskDescription
        );
        let was_add_task_picker = matches!(
            &overlay,
            OverlayState::Picker(state)
                if matches!(
                    state.route,
                    OverlayRoute::AddTaskTitleProject | OverlayRoute::AddTaskTitlePriority
                )
        ) || matches!(
            &overlay,
            OverlayState::TagCombobox(state) if state.route == OverlayRoute::AddTaskTitleLabels
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
            OverlayOutcome::Cancelled if was_detail_help => {
                self.overlay = Some(OverlayState::Detail { scroll: 0 })
            }
            OverlayOutcome::Cancelled if was_add_task_description_editor || was_add_task_picker => {
                self.begin_add_task_step()
            }
            OverlayOutcome::Cancelled if self.add_task_only => self.should_quit = true,
            OverlayOutcome::Cancelled => self.cancel_authoring_overlay(),
            OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
        }
        Ok(())
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
                if self.go_back_in_detail() {
                    return Ok(Some(self.overlay.take()));
                }
                self.detail_context = false;
                self.detail_context_scroll = 0;
                Ok(Some(None))
            }
            DetailShortcutResolution::Action(action) => {
                self.pending_shortcut_scroll = 0;
                self.detail_context = true;
                self.detail_context_scroll = scroll;
                self.execute(action).await?;
                if self.detail_context && self.overlay.is_none() {
                    self.restore_detail_overlay_at_scroll(true, scroll);
                }
                Ok(Some(self.overlay.take()))
            }
            DetailShortcutResolution::Prefix => {
                self.pending_shortcut_scroll = 0;
                Ok(Some(Some(OverlayState::Detail { scroll })))
            }
            DetailShortcutResolution::MissingAfterPrefix(label) => {
                self.pending_shortcut_scroll = 0;
                self.set_warning(format!("invalid shortcut: {label}"));
                Ok(Some(Some(OverlayState::Detail { scroll })))
            }
            DetailShortcutResolution::PassThrough => Ok(None),
        }
    }

    async fn handle(&mut self, action: Action) -> Result<()> {
        self.execute(action).await
    }

    pub(super) async fn execute(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Quit => self.should_quit = true,
            Action::CancelOverlay => self.cancel_overlay(),
            Action::MoveDown => self.move_selection(1).await?,
            Action::MoveUp => self.move_selection(-1).await?,
            Action::MoveLeft => self.move_left(),
            Action::MoveRight => self.move_right(),
            Action::MoveColumnLeft => self.move_tasks_by_column(-1).await?,
            Action::MoveColumnRight => self.move_tasks_by_column(1).await?,
            Action::BeginMoveToColumn => self.begin_move_to_column(),
            Action::PreviousItem => self.previous_item(),
            Action::NextItem => self.next_item(),
            Action::First => self.select_edge(false).await?,
            Action::Last => self.select_edge(true).await?,
            Action::ToggleFocus => self.toggle_focus(),
            Action::ToggleSidebar => self.toggle_sidebar(),
            Action::ToggleDetail => self.activate_or_toggle_detail().await?,
            Action::ToggleColumnsPreview => {
                if self.store.view_state.view == crate::tui::store::TaskView::Columns {
                    self.store.columns_preview_visible = !self.store.columns_preview_visible;
                }
            }
            Action::GoBack => self.go_back().await?,
            Action::ToggleHelp => self.toggle_help_at_height(24),
            Action::BeginSearch => self.begin_search(),
            Action::BeginCommand => self.begin_command(),
            Action::Refresh => self.refresh().await?,
            Action::SetOrder(order) => self.set_sort(order).await?,
            Action::ReverseSort => self.reverse_sort().await?,
            Action::SetStatus(status) => self.update_status(status).await?,
            Action::SetPriority(priority) => self.set_exact_priority(priority).await?,
            Action::CyclePriority(reverse) => self.update_priority(reverse).await?,
            Action::CopyShortRef => self.copy_selected_ref(TaskRefKind::Short),
            Action::CopyDurableRef => self.copy_selected_ref(TaskRefKind::Durable),
            Action::CopyTaskTitle => self.copy_selected_task_text(TaskCopyKind::Title),
            Action::CopyTaskDescription => self.copy_selected_task_text(TaskCopyKind::Description),
            Action::CopyTaskText => self.copy_selected_task_text(TaskCopyKind::TitleAndDescription),
            Action::CopyTaskNotes => self.copy_selected_task_notes(),
            Action::BeginEditTitle => self.begin_edit_title(),
            Action::BeginEditDescription => self.begin_edit_description(),
            Action::BeginEditProject => self.begin_edit_project(),
            Action::BeginEditPriority => self.begin_edit_priority(),
            Action::BeginEditAvailability => self.begin_edit_availability(),
            Action::BeginEditDue => self.begin_edit_due(),
            Action::BeginEditLabels => self.begin_edit_labels(),
            Action::Delete => self.begin_delete_task(),
            Action::Restore => self.update_deleted(false).await?,
            Action::BeginStatusPicker => self.begin_status_picker(),
            Action::BeginRenameProject => self.begin_rename_project(),
            Action::BeginDeleteProject => self.begin_delete_project(),
            Action::BeginAddProject => self.begin_add_project(),
            Action::BeginAddLabel => self.begin_add_label(),
            Action::BeginAddTask => self.begin_add_task().await?,
            Action::BeginAddNote => self.begin_add_note(),
            Action::BeginFilterLabel => self.begin_filter_label(),
            Action::BeginFilterPriority => self.begin_filter_priority(),
            Action::BeginScopeProject => self.begin_scope_project(),
            Action::BeginSwitchWorkspace => self.begin_switch_workspace().await?,
            Action::ClearFilters => self.clear_filters().await?,
            Action::ToggleDeletedFilter => self.toggle_deleted_filter().await?,
            Action::ShowView(view) => self.show_view(view).await?,
            Action::ShowWorkspaceScope => {
                self.show_scope(crate::tui::store::TaskScopeTarget::Workspace)
                    .await?
            }
            Action::BeginConflictList => self.open_conflict_list().await?,
            Action::ShowConflictDetails => self.show_conflict_details().await?,
            Action::NextConflict => self.move_to_conflict(1),
            Action::PreviousConflict => self.move_to_conflict(-1),
            Action::AcceptConflictLocal => {
                self.begin_conflict_resolution(ConflictResolutionChoice::Local)
                    .await?
            }
            Action::AcceptConflictRemote => {
                self.begin_conflict_resolution(ConflictResolutionChoice::Remote)
                    .await?
            }
            Action::BeginManualConflictMerge => self.begin_manual_conflict_merge().await?,
            Action::ShowConfigStatus => self.show_config_status()?,
            Action::ShowConfigInfo => self.show_config_info()?,
            Action::ShowConfigPaths => self.show_config_paths()?,
            Action::ShowDatabaseStats => self.show_database_stats().await?,
            Action::BeginUpdate => self.begin_update(),
            Action::BeginConfigInit => self.begin_config_init()?,
            Action::BeginAddDependency => self.begin_add_dependency().await?,
            Action::BeginRemoveDependency => self.begin_remove_dependency(),
            Action::Undo => self.undo_last().await?,
            Action::ToggleMarkSelected => self.toggle_mark_selected(),
            Action::ToggleMarkAllInView => self.toggle_mark_all_in_view(),
            Action::ClearMarks => self.clear_marks(),
            Action::ToggleEpicExpanded => {
                let index = self.widgets.table.selected();
                if let Some(message) = self.store.toggle_selected_epic(index).await? {
                    self.set_info(message.message);
                    self.widgets.table.select(message.selected);
                }
            }
            Action::DetachEpicChild => {
                let index = self.widgets.table.selected();
                if let Some(message) = self.store.detach_selected_epic_child(index).await? {
                    self.set_info(message.message);
                    self.widgets.table.select(message.selected);
                }
            }
            Action::PromoteEpicChild => {
                let index = self.widgets.table.selected();
                if let Some(message) = self.store.promote_selected_epic_child(index).await? {
                    self.set_info(message.message);
                    self.widgets.table.select(message.selected);
                }
            }
            Action::Planned { name, reason } => {
                self.set_warning(format!(":{name} is not yet implemented: {reason}"));
            }
            Action::Disabled { name, reason } => {
                self.set_warning(format!(":{name} is disabled: {reason}"));
            }
            Action::AcceptCommand
            | Action::CancelCommand
            | Action::BackspaceCommand
            | Action::CommandChar(_)
            | Action::AcceptSearch
            | Action::CancelSearch
            | Action::BackspaceSearch
            | Action::SearchChar(_)
            | Action::None => {}
        }
        Ok(())
    }

    fn accept_command_input(&mut self, input: &str) -> Option<Action> {
        match lookup_command(input) {
            CommandLookup::Found(action) => {
                self.pending_shortcut.clear();
                Some(action)
            }
            CommandLookup::Empty => {
                self.set_info("empty command");
                None
            }
            CommandLookup::Ambiguous => {
                self.set_warning(format!("ambiguous command: {}", input.trim()));
                None
            }
            CommandLookup::Missing => {
                self.set_warning(format!("unknown command: {}", input.trim()));
                None
            }
        }
    }

    fn complete_command_input(&mut self, state: &mut CommandState, reverse: bool) {
        let cycle_input = state
            .cycle_input
            .clone()
            .unwrap_or_else(|| state.input.text.clone());
        let options = command_cycle_options(&cycle_input);
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
            self.set_info(format!(
                "command {} of {}",
                state.cycle_index + 1,
                options.len()
            ));
            return;
        }

        let highlighted = state.highlighted.clone();
        state.reset_cycle();
        state.highlighted = highlighted;
        match complete_command(state.input.as_str()) {
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
            CommandCompletion::Unchanged => self.set_info("no further completion"),
        }
    }

    pub(super) fn toggle_help_at_height(&mut self, _terminal_height: u16) {
        match self.overlay {
            Some(OverlayState::Help { .. }) => self.overlay = None,
            Some(OverlayState::DetailHelp { .. }) => {
                self.overlay = Some(OverlayState::Detail { scroll: 0 })
            }
            Some(OverlayState::Detail { .. }) => {
                self.overlay = Some(OverlayState::DetailHelp { scroll: 0 })
            }
            _ => self.overlay = Some(OverlayState::Help { scroll: 0 }),
        }
    }
}

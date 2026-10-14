use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Size;

use crate::tui::app::{App, DetailSection, DetailTargetId, FooterChoiceMode};
use crate::tui::detail_session::DetailTargetActivation;
use crate::tui::event::{Action, CommandHandler, DetailFocusPolicy};
use crate::tui::navigation::{
    detail_task_delta, handle_detail_scroll_key_with_cap, handle_detail_scroll_key_with_images,
};
use crate::tui::overlay::{ConfirmIntent, OverlayState};
use crate::tui::platform::copy_to_clipboard;
use crate::tui::shortcut_buffer::DetailShortcutResolution;
use crate::tui::store::TaskQuery;
use crate::tui::ui::detail_copy_target_at;

impl App {
    pub(super) fn open_detail_attachment(&mut self, attachment_id: String, scroll: u16) {
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

    pub(super) async fn handle_detail_target_mouse_click(
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
                    Some(DetailTargetActivation::EditMetadata) => {
                        self.begin_edit_metadata().await?;
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

    pub(super) async fn handle_detail_mouse_click(
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

    pub(super) fn handle_detail_mouse_move(&mut self, mouse: MouseEvent, terminal_size: Size) {
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

    pub(super) fn detail_focus_warning(&self) -> &'static str {
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
            Some(DetailTargetId::CustomMetadata) => {
                "leave metadata focus before using that command"
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

    pub(super) async fn handle_focused_detail_shortcut(
        &mut self,
        key: KeyEvent,
        scroll: u16,
        terminal_size: Size,
    ) -> Result<bool> {
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
        if target == DetailTargetId::CustomMetadata {
            if let Some(outcome) = self
                .handle_detail_shortcut(key, scroll, terminal_size)
                .await?
            {
                self.overlay = outcome;
                return Ok(true);
            }
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
            DetailShortcutResolution::Action(action @ (Action::First | Action::Last)) => {
                self.jump_detail_scroll(action, terminal_size);
                Ok(true)
            }
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

    pub(super) async fn execute_focused_detail_action(
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

    pub(super) async fn handle_detail_shortcut(
        &mut self,
        key: KeyEvent,
        scroll: u16,
        terminal_size: Size,
    ) -> Result<Option<Option<OverlayState>>> {
        if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
            return Ok(None);
        }

        match self.pending_shortcut.resolve_detail_in_domain(
            key,
            &self.command_catalog,
            crate::tui::event::RoutingDomain::DetailParent,
        ) {
            DetailShortcutResolution::Action(action @ (Action::First | Action::Last)) => {
                self.jump_detail_scroll(action, terminal_size);
                Ok(Some(self.overlay.take()))
            }
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

    fn jump_detail_scroll(&mut self, action: Action, terminal_size: Size) {
        self.pending_shortcut_scroll = 0;
        let scroll = if action == Action::First {
            0
        } else {
            self.detail_document_for_query(terminal_size)
                .map_or(0, |document| document.scroll_cap())
        };
        if let Some(detail) = self.detail.state_mut() {
            detail.set_focused_target(None);
        }
        self.show_detail(scroll);
    }

    pub(super) async fn handle_detail_overlay_key(
        &mut self,
        key: KeyEvent,
        terminal_size: Size,
    ) -> Result<()> {
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
            && self
                .command_catalog
                .custom_shortcut_starts_with(crate::tui::event::CommandContext::Detail, &[key.code])
            && self
                .handle_focused_detail_shortcut(key, scroll, terminal_size)
                .await?
        {
            return Ok(());
        }
        if !self.pending_shortcut.is_empty()
            && self
                .handle_focused_detail_shortcut(key, scroll, terminal_size)
                .await?
        {
            return Ok(());
        }
        if (!self.pending_shortcut.is_empty() || key.code == KeyCode::Char('g'))
            && let Some(outcome) = self
                .handle_detail_shortcut(key, scroll, terminal_size)
                .await?
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
                (KeyCode::Char('e'), KeyModifiers::NONE)
                    if matches!(&selected_target, DetailTargetId::Note { .. }) =>
                {
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
                    DetailTargetId::CustomMetadata => {
                        self.begin_edit_metadata().await?;
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
                    if self
                        .handle_focused_detail_shortcut(key, scroll, terminal_size)
                        .await?
                    {
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

        if let Some(outcome) = self
            .handle_detail_shortcut(key, scroll, terminal_size)
            .await?
        {
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

        let document = self.detail_document_for_query(terminal_size);
        let scroll = if let Some(document) = document {
            handle_detail_scroll_key_with_cap(
                key,
                scroll,
                terminal_size.height,
                document.scroll_cap(),
            )
        } else {
            let task = self.store.selected_task(self.list.selected_task());
            handle_detail_scroll_key_with_images(
                &self.theme,
                key,
                scroll,
                terminal_size.width,
                terminal_size.height,
                task,
                inline_images.as_ref(),
            )
        };
        self.show_detail(scroll);
        Ok(())
    }
}

use anyhow::Result;
use crossterm::event::MouseEvent;
use ratatui::layout::{Rect, Size};

use crate::tui::app::{App, DetailSection, DetailTargetId};
use crate::tui::overlay::{OverlayView, TextInputKind};
use crate::tui::ui::{
    DetailRenderContext, attachment_is_locally_previewable, detail_selected_text,
    detail_target_is_actionable,
};

impl App {
    pub(super) fn detail_document_for_query(
        &self,
        terminal_size: Size,
    ) -> Option<std::rc::Rc<crate::tui::ui::DetailDocument>> {
        let item = self.store.selected_task(self.list.selected_task())?;
        let overlay = self.overlay.as_ref().map(OverlayView::from);
        let inline_title_editor = match overlay.as_ref() {
            Some(OverlayView::TextInput(state)) if state.kind == TextInputKind::EditTitle => {
                Some(state)
            }
            _ => None,
        };
        let detail = self.detail.state()?;
        let inline_images = self.inline_image_context();
        let pending_attachments = self.attachment_controller.views();
        let context = DetailRenderContext {
            terminal_area: Rect::new(0, 0, terminal_size.width, terminal_size.height),
            scroll: detail.scroll(),
            inline_title_editor,
            active_target: detail.focused_target(),
            hovered_target: detail.hovered_target(),
            expanded_sections: detail.expanded_sections(),
            selection: detail.text_selection(),
            inline_images: inline_images.as_ref(),
            pending_attachments: &pending_attachments,
        };
        if let Some(document) = self
            .widgets
            .detail_document
            .as_ref()
            .filter(|document| document.matches_frame(item, &context))
        {
            return Some(std::rc::Rc::clone(document));
        }
        Some(std::rc::Rc::new(crate::tui::ui::DetailDocument::build(
            item, &context,
        )))
    }

    pub(super) fn begin_detail_text_selection(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> bool {
        if self.detail.is_inactive() || self.overlay.is_some() {
            return false;
        }
        let Some(item) = self.store.selected_task(self.list.selected_task()) else {
            return false;
        };
        let cell = self
            .detail_document_for_query(terminal_size)
            .and_then(|document| document.text_cell_at_position(mouse.column, mouse.row));
        let Some(cell) = cell else {
            return false;
        };
        if let Some(detail) = self.detail.state_mut() {
            detail.begin_text_selection(item.task.id.clone(), terminal_size.width, cell);
        }
        self.list.clear_task_click();
        true
    }

    pub(super) fn update_detail_text_selection(&mut self, mouse: MouseEvent, terminal_size: Size) {
        if !self
            .detail
            .state()
            .is_some_and(|detail| detail.text_dragging())
        {
            return;
        }
        if self.detail.is_inactive() || self.overlay.is_some() {
            return;
        }
        let Some(item) = self.store.selected_task(self.list.selected_task()) else {
            return;
        };
        let cell = self
            .detail_document_for_query(terminal_size)
            .and_then(|document| document.text_cell_at_position(mouse.column, mouse.row));
        let Some(cell) = cell else {
            return;
        };
        if let Some(detail) = self.detail.state_mut() {
            detail.update_text_selection(&item.task.id, terminal_size.width, cell);
        }
    }

    pub(super) fn copy_detail_text_selection(&mut self) {
        let value = self
            .detail
            .state()
            .and_then(|detail| detail.text_selection())
            .and_then(|selection| {
                self.widgets
                    .detail_document
                    .as_ref()
                    .and_then(|document| document.selected_text(selection))
                    .or_else(|| {
                        self.store
                            .selected_task(self.list.selected_task())
                            .and_then(|item| detail_selected_text(item, selection))
                    })
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

    pub(super) fn detail_focus_targets(&self, terminal_size: Size) -> Vec<DetailTargetId> {
        let Some(item) = self.store.selected_task(self.list.selected_task()) else {
            return Vec::new();
        };
        let rows = self
            .detail_document_for_query(terminal_size)
            .map(|document| document.interactive_rows().to_vec())
            .unwrap_or_default();
        let mut targets = rows
            .into_iter()
            .map(|row| row.target)
            .filter(|target| detail_target_is_actionable(item, target))
            .collect::<Vec<_>>();
        if let Some(removed) = self
            .detail
            .state()
            .and_then(|detail| detail.removed_epic_child())
            && removed.epic_id == item.task.id
            && !item
                .epic_children
                .iter()
                .any(|child| child.task_id == removed.child.task_id)
        {
            let removed_target = DetailTargetId::Task {
                section: DetailSection::EpicChildren,
                task_id: removed.child.task_id.clone(),
            };
            let child_indices = targets
                .iter()
                .enumerate()
                .filter_map(|(index, target)| {
                    (target.section() == DetailSection::EpicChildren).then_some(index)
                })
                .collect::<Vec<_>>();
            let insert_at = child_indices
                .get(removed.original_position)
                .copied()
                .or_else(|| child_indices.last().map(|index| index + 1))
                .unwrap_or_else(|| {
                    targets
                        .iter()
                        .position(|target| target.section() > DetailSection::EpicChildren)
                        .unwrap_or(targets.len())
                });
            targets.insert(insert_at, removed_target);
        }
        targets
    }

    pub(super) fn selected_detail_focus_target(
        &self,
        terminal_size: Size,
    ) -> Option<DetailTargetId> {
        let targets = self.detail_focus_targets(terminal_size);
        self.detail
            .state()
            .and_then(|detail| detail.selected_target(&targets))
    }

    pub(super) fn focus_detail_section(&mut self, reverse: bool, terminal_size: Size) -> bool {
        let targets = self.detail_focus_targets(terminal_size);
        self.detail
            .state_mut()
            .is_some_and(|detail| detail.focus_section(&targets, reverse))
    }

    pub(super) fn move_detail_focus_selection(
        &mut self,
        delta: isize,
        terminal_size: Size,
    ) -> bool {
        let targets = self.detail_focus_targets(terminal_size);
        self.detail
            .state_mut()
            .is_some_and(|detail| detail.move_focus(&targets, delta))
    }

    pub(super) fn move_attachment_preview_selection(
        &mut self,
        attachment_id: &str,
        delta: isize,
    ) -> Option<String> {
        let context = self.inline_image_context()?;
        if !context.previews_enabled {
            return None;
        }
        let attachment_ids = self
            .store
            .selected_task(self.list.selected_task())?
            .attachments
            .iter()
            .filter(|attachment| {
                attachment_is_locally_previewable(attachment, &context.unavailable_hashes)
            })
            .map(|attachment| attachment.attachment_id.clone())
            .collect::<Vec<_>>();
        let index = attachment_ids
            .iter()
            .position(|candidate| candidate == attachment_id)?;
        let next = (index as isize + delta).rem_euclid(attachment_ids.len() as isize) as usize;
        let attachment_id = attachment_ids[next].clone();
        if let Some(detail) = self.detail.state_mut() {
            detail.set_focused_target(Some(DetailTargetId::Attachment {
                attachment_id: attachment_id.clone(),
            }));
        }
        Some(attachment_id)
    }

    pub(super) fn detail_focus_scroll(&self, scroll: u16, terminal_size: Size) -> u16 {
        let Some(target) = self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
        else {
            return scroll;
        };
        self.detail_document_for_query(terminal_size)
            .and_then(|document| document.target_scroll_target(target, scroll))
            .unwrap_or(scroll)
    }

    pub(super) fn activate_detail_disclosure(
        &mut self,
        section: DetailSection,
        terminal_size: Size,
    ) {
        let first_revealed_index = self
            .detail_focus_targets(terminal_size)
            .into_iter()
            .filter(|target| {
                target.section() == section && matches!(target, DetailTargetId::Task { .. })
            })
            .count();
        let expanded = self
            .detail
            .state_mut()
            .is_some_and(|detail| detail.toggle_section(section));
        if !expanded {
            return;
        }
        let target = self
            .detail_focus_targets(terminal_size)
            .into_iter()
            .filter(|target| {
                target.section() == section && matches!(target, DetailTargetId::Task { .. })
            })
            .nth(first_revealed_index)
            .or(Some(DetailTargetId::Expand { section }));
        if let Some(detail) = self.detail.state_mut() {
            detail.set_focused_target(target);
        }
    }

    pub(super) fn detail_attachment_supports_inline_preview(&self, attachment_id: &str) -> bool {
        let Some(context) = self.inline_image_context() else {
            return false;
        };
        context.previews_enabled
            && self
                .store
                .selected_task(self.list.selected_task())
                .and_then(|item| {
                    item.attachments
                        .iter()
                        .find(|attachment| attachment.attachment_id == attachment_id)
                })
                .is_some_and(|attachment| {
                    attachment_is_locally_previewable(attachment, &context.unavailable_hashes)
                })
    }

    pub(super) async fn close_detail_session(&mut self) -> Result<()> {
        self.clear_detail_session();
        if !self.restore_recent_action_return().await? && !self.restore_last_change_return().await?
        {
            self.refresh().await?;
        }
        Ok(())
    }

    pub(super) async fn navigate_back_from_detail(&mut self) -> Result<()> {
        if self.list.has_recent_action_return()
            || self.list.has_last_change_return()
            || !self.go_back_in_detail().await?
        {
            self.close_detail_session().await?;
        }
        Ok(())
    }

    pub(super) async fn open_detail_task(&mut self, task_id: &crate::ids::TaskId, scroll: u16) {
        let current_task_id = self
            .store
            .selected_task(self.list.selected_task())
            .map(|item| item.task.id.clone());
        self.show_detail(scroll);
        let item = match self.store.load_task_item(task_id).await {
            Ok(Some(item)) => item,
            Ok(None) => {
                self.set_warning("linked task is unavailable");
                return;
            }
            Err(_) => {
                self.set_warning("could not load linked task");
                return;
            }
        };
        if let Some(current_task_id) = current_task_id.clone() {
            let previous = self
                .detail
                .state()
                .expect("show_detail establishes an active detail session")
                .snapshot(current_task_id, self.store.view_state.clone());
            if let Some(detail) = self.detail.state_mut() {
                detail.follow_link(previous);
            }
        }
        self.store.show_exact_task(item);
        self.list.select_task(Some(0));
        self.list.clear_task_click();
        if current_task_id.is_none()
            && let Some(detail) = self.detail.state_mut()
        {
            detail.reset_task_state(0);
        } else if self.detail.is_inactive() {
            self.detail = crate::tui::detail_session::DetailSession::open(0);
        }
        self.show_detail(0);
    }
}

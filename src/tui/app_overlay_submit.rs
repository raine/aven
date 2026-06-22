use anyhow::Result;

use crate::tui::app::App;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{OverlayRoute, OverlaySubmit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextSubmitRoute {
    AddTaskTitleToast,
    AddProject,
    AddLabel,
    EditTitle,
    ConflictManual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MultilineSubmitRoute {
    AddTaskDescription,
    AddTaskNatural,
    AddNote,
    EditDescription,
    ConflictManual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerSubmitRoute {
    AddTaskTitleProject,
    AddTaskTitlePriority,
    EditStatus,
    EditProject,
    EditPriority,
    EditLabels,
    FilterProject,
    FilterLabel,
    FilterStatus,
    FilterPriority,
    ViewProject,
    DeleteProjectPicker,
    SwitchWorkspace,
    ConflictField,
    ConflictManual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmSubmitRoute {
    ConflictConfirm,
    ConfigInit,
    DeleteProjectConfirm,
    DeleteTaskConfirm,
}

fn text_submit_route(route: OverlayRoute) -> Option<TextSubmitRoute> {
    match route {
        OverlayRoute::AddTaskTitle => Some(TextSubmitRoute::AddTaskTitleToast),
        OverlayRoute::AddProject => Some(TextSubmitRoute::AddProject),
        OverlayRoute::AddLabel => Some(TextSubmitRoute::AddLabel),
        OverlayRoute::EditTitle => Some(TextSubmitRoute::EditTitle),
        OverlayRoute::ConflictManual => Some(TextSubmitRoute::ConflictManual),
        _ => None,
    }
}

fn multiline_submit_route(route: OverlayRoute) -> Option<MultilineSubmitRoute> {
    match route {
        OverlayRoute::AddTaskDescription => Some(MultilineSubmitRoute::AddTaskDescription),
        OverlayRoute::AddTaskNatural => Some(MultilineSubmitRoute::AddTaskNatural),
        OverlayRoute::AddNote => Some(MultilineSubmitRoute::AddNote),
        OverlayRoute::EditDescription => Some(MultilineSubmitRoute::EditDescription),
        OverlayRoute::ConflictManual => Some(MultilineSubmitRoute::ConflictManual),
        _ => None,
    }
}

fn picker_submit_route(route: OverlayRoute) -> Option<PickerSubmitRoute> {
    match route {
        OverlayRoute::AddTaskTitleProject => Some(PickerSubmitRoute::AddTaskTitleProject),
        OverlayRoute::AddTaskTitlePriority => Some(PickerSubmitRoute::AddTaskTitlePriority),
        OverlayRoute::EditStatus => Some(PickerSubmitRoute::EditStatus),
        OverlayRoute::EditProject => Some(PickerSubmitRoute::EditProject),
        OverlayRoute::EditPriority => Some(PickerSubmitRoute::EditPriority),
        OverlayRoute::EditLabels => Some(PickerSubmitRoute::EditLabels),
        OverlayRoute::FilterProject => Some(PickerSubmitRoute::FilterProject),
        OverlayRoute::FilterLabel => Some(PickerSubmitRoute::FilterLabel),
        OverlayRoute::FilterStatus => Some(PickerSubmitRoute::FilterStatus),
        OverlayRoute::FilterPriority => Some(PickerSubmitRoute::FilterPriority),
        OverlayRoute::ViewProject => Some(PickerSubmitRoute::ViewProject),
        OverlayRoute::DeleteProjectPicker => Some(PickerSubmitRoute::DeleteProjectPicker),
        OverlayRoute::SwitchWorkspace => Some(PickerSubmitRoute::SwitchWorkspace),
        OverlayRoute::ConflictField => Some(PickerSubmitRoute::ConflictField),
        OverlayRoute::ConflictManual => Some(PickerSubmitRoute::ConflictManual),
        _ => None,
    }
}

fn confirm_submit_route(route: OverlayRoute) -> Option<ConfirmSubmitRoute> {
    match route {
        OverlayRoute::ConflictConfirm => Some(ConfirmSubmitRoute::ConflictConfirm),
        OverlayRoute::ConfigInit => Some(ConfirmSubmitRoute::ConfigInit),
        OverlayRoute::DeleteProjectConfirm => Some(ConfirmSubmitRoute::DeleteProjectConfirm),
        OverlayRoute::DeleteTaskConfirm => Some(ConfirmSubmitRoute::DeleteTaskConfirm),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn handles_submit_kind(
    route: OverlayRoute,
    kind: crate::tui::overlay::OverlaySubmitKind,
) -> bool {
    use crate::tui::overlay::OverlaySubmitKind::{Confirm, Multiline, Picker, Text};

    match kind {
        Text => text_submit_route(route).is_some(),
        Multiline => multiline_submit_route(route).is_some(),
        Picker => picker_submit_route(route).is_some(),
        Confirm => confirm_submit_route(route).is_some(),
    }
}

impl App {
    pub(super) async fn handle_overlay_submit(&mut self, submit: OverlaySubmit) -> Result<()> {
        match submit {
            OverlaySubmit::AddTask { title, description } => {
                self.handle_add_task_submit(title, description).await?;
            }
            OverlaySubmit::Picker {
                route,
                title,
                values,
            } => {
                self.handle_picker_submit(route, title, values).await?;
            }
            OverlaySubmit::Text {
                route,
                title,
                value,
            } => {
                self.handle_text_submit(route, title, value).await?;
            }
            OverlaySubmit::Multiline {
                route,
                title,
                value,
            } => {
                self.handle_multiline_submit(route, title, value).await?;
            }
            OverlaySubmit::Confirm { route, title } => {
                self.handle_confirm_submit(route, title).await?;
            }
        }
        Ok(())
    }

    async fn handle_add_task_submit(&mut self, title: String, description: String) -> Result<()> {
        self.authoring
            .capture_add_task_fields(title, description, AddTaskStep::Title);
        self.submit_add_task_from_authoring().await
    }

    async fn handle_text_submit(
        &mut self,
        route: OverlayRoute,
        title: String,
        value: String,
    ) -> Result<()> {
        match text_submit_route(route) {
            Some(TextSubmitRoute::AddTaskTitleToast) => self.set_success(
                OverlaySubmit::Text {
                    route,
                    title,
                    value,
                }
                .message(),
            ),
            Some(TextSubmitRoute::AddProject) => {
                let message = self.store.create_project(value).await?;
                self.restore_selection_after_mutation();
                self.set_success(message);
            }
            Some(TextSubmitRoute::AddLabel) => {
                let message = self.store.create_label(value).await?;
                self.set_success(message);
            }
            Some(TextSubmitRoute::EditTitle) => {
                self.submit_edit_title(value).await?;
            }
            Some(TextSubmitRoute::ConflictManual) => {
                self.submit_manual_conflict_value(value).await?;
            }
            None => self.set_success(
                OverlaySubmit::Text {
                    route,
                    title,
                    value,
                }
                .message(),
            ),
        }
        Ok(())
    }

    async fn handle_multiline_submit(
        &mut self,
        route: OverlayRoute,
        title: String,
        value: String,
    ) -> Result<()> {
        match multiline_submit_route(route) {
            Some(MultilineSubmitRoute::AddTaskDescription) => {
                if self.authoring.capture_add_task_fields(
                    self.authoring
                        .add_task_context()
                        .map(|context| context.title)
                        .unwrap_or_default(),
                    value,
                    AddTaskStep::Description,
                ) {
                    self.begin_add_task_step();
                }
            }
            Some(MultilineSubmitRoute::AddTaskNatural) => {
                self.submit_add_task_natural(value).await?;
            }
            Some(MultilineSubmitRoute::AddNote) => {
                self.submit_add_note(value).await?;
            }
            Some(MultilineSubmitRoute::EditDescription) => {
                self.submit_edit_description(value).await?;
            }
            Some(MultilineSubmitRoute::ConflictManual) => {
                self.submit_manual_conflict_value(value).await?;
            }
            None => self.set_success(
                OverlaySubmit::Multiline {
                    route,
                    title,
                    value,
                }
                .message(),
            ),
        }
        Ok(())
    }

    async fn handle_picker_submit(
        &mut self,
        route: OverlayRoute,
        title: String,
        values: Vec<String>,
    ) -> Result<()> {
        match picker_submit_route(route) {
            Some(PickerSubmitRoute::AddTaskTitleProject) => {
                if self.authoring.apply_add_task_project(values) {
                    self.begin_add_task_step();
                }
            }
            Some(PickerSubmitRoute::AddTaskTitlePriority) => {
                if self.authoring.apply_add_task_priority(values) {
                    self.begin_add_task_step();
                }
            }
            Some(PickerSubmitRoute::EditStatus) => match values.first() {
                Some(status) => self.submit_edit_status(status.clone()).await?,
                None => {
                    self.set_warning("no matching status");
                    self.begin_status_picker();
                }
            },
            Some(PickerSubmitRoute::EditProject) => match values.first() {
                Some(project) => self.submit_edit_project(project.clone()).await?,
                None => {
                    self.set_warning("no matching project");
                    self.begin_edit_project();
                }
            },
            Some(PickerSubmitRoute::EditPriority) => match values.first() {
                Some(priority) => self.submit_edit_priority(priority.clone()).await?,
                None => {
                    self.set_warning("no matching priority");
                    self.begin_edit_priority();
                }
            },
            Some(PickerSubmitRoute::EditLabels) => {
                self.submit_edit_labels(values).await?;
            }
            Some(PickerSubmitRoute::FilterProject) => {
                self.submit_filter_project(values).await?;
            }
            Some(PickerSubmitRoute::FilterLabel) => {
                self.submit_filter_label(values).await?;
            }
            Some(PickerSubmitRoute::FilterStatus) => {
                self.submit_filter_status(values).await?;
            }
            Some(PickerSubmitRoute::FilterPriority) => {
                self.submit_filter_priority(values).await?;
            }
            Some(PickerSubmitRoute::ViewProject) => {
                self.submit_view_project(values).await?;
            }
            Some(PickerSubmitRoute::DeleteProjectPicker) => {
                self.submit_delete_project_picker(values);
            }
            Some(PickerSubmitRoute::SwitchWorkspace) => {
                self.submit_switch_workspace(values).await?;
            }
            Some(PickerSubmitRoute::ConflictField) => {
                self.submit_conflict_field_picker(values).await?;
            }
            Some(PickerSubmitRoute::ConflictManual) => {
                if let Some(value) = values.first() {
                    self.submit_manual_conflict_value(value.clone()).await?;
                } else {
                    self.set_warning("no value selected");
                }
            }
            None => self.set_success(
                OverlaySubmit::Picker {
                    route,
                    title,
                    values,
                }
                .message(),
            ),
        }
        Ok(())
    }

    async fn handle_confirm_submit(&mut self, route: OverlayRoute, title: String) -> Result<()> {
        match confirm_submit_route(route) {
            Some(ConfirmSubmitRoute::ConflictConfirm) => {
                self.submit_confirmed_conflict_resolution().await?;
            }
            Some(ConfirmSubmitRoute::ConfigInit) => {
                self.submit_config_init()?;
            }
            Some(ConfirmSubmitRoute::DeleteProjectConfirm) => {
                self.submit_delete_project().await?;
            }
            Some(ConfirmSubmitRoute::DeleteTaskConfirm) => {
                let return_to_detail = self.detail_context;
                self.update_deleted(true).await?;
                self.detail_context = false;
                self.restore_detail_overlay(return_to_detail);
            }
            None => self.set_success(OverlaySubmit::Confirm { route, title }.message()),
        }
        Ok(())
    }
}

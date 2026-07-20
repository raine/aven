use anyhow::Result;

use crate::tui::app::{App, TaskCopyKind, TaskRefKind};
use crate::tui::conflict_flow::ConflictResolutionChoice;
use crate::tui::event::Action;

impl App {
    pub(in crate::tui) async fn execute(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Quit => self.should_quit = true,
            Action::CancelOverlay => self.cancel_overlay(),
            Action::MoveDown => self.move_selection(1).await?,
            Action::MoveUp => self.move_selection(-1).await?,
            Action::MoveLeft => self.move_left(),
            Action::MoveRight => self.move_right().await?,
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
            Action::ReturnToLastChange => self.return_to_last_change().await?,
            Action::ToggleHelp => self.toggle_help_at_height(24),
            Action::ShowWelcome => self.show_welcome(),
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
            Action::SkipRecurrence => self.skip_recurrence().await?,
            Action::BeginRecordRecurrence => self.begin_record_recurrence(),
            Action::BeginEditRecurrenceTemplate => self.begin_edit_recurrence_template().await?,
            Action::PauseRecurrence => self.pause_recurrence().await?,
            Action::ResumeRecurrence => self.resume_recurrence().await?,
            Action::StopRecurrence => self.stop_recurrence().await?,
            Action::ShowRecurrenceHistory => self.show_recurrence_history().await?,
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
            Action::ToggleClosedFilter => self.toggle_closed_filter().await?,
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
                let index = self.list.selected_task();
                if let Some(message) = self.store.toggle_selected_epic(index).await? {
                    self.set_info(message.message);
                    self.list.select_task(message.selected);
                } else {
                    self.set_warning("Select an epic in the Epics list");
                }
            }
            Action::BeginAddEpicChild => self.begin_add_epic_child(),
            Action::RemoveEpicChild => self.remove_selected_epic_child().await?,
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
}

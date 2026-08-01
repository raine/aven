use crate::query::TaskDependencyLink;
use crate::tui::store::MutationMessage;
use crate::undo::UndoContext;

use super::{TaskListRenderMode, TuiStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EpicContext {
    pub(crate) epic_id: crate::ids::TaskId,
    pub(crate) display_ref: String,
    pub(crate) project_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddEpicChildContext {
    Existing(EpicContext),
    Promote(EpicContext),
}

#[derive(Debug, Clone)]
pub(crate) struct EpicChildTarget {
    pub(crate) epic: EpicContext,
    pub(crate) child: TaskDependencyLink,
    pub(crate) original_position: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct EpicChildMutation {
    pub(crate) message: MutationMessage,
    pub(crate) epic: EpicContext,
    pub(crate) child: TaskDependencyLink,
    pub(crate) original_position: usize,
    pub(crate) changed: bool,
}

impl TuiStore {
    pub(crate) async fn toggle_selected_epic(
        &mut self,
        index: Option<usize>,
    ) -> Result<Option<MutationMessage>, anyhow::Error> {
        if self.view_state.render_mode() != TaskListRenderMode::Epics {
            return Ok(None);
        }
        let (task_id, display_ref) = {
            let Some(item) = self.selected_task(index) else {
                return Ok(None);
            };
            if !item.task.is_epic {
                return Ok(None);
            }
            (item.task.id.clone(), item.display_ref.clone())
        };
        let mut view_state = self.view_state.clone();
        let message = if view_state.expanded_epic_ids.contains(&task_id) {
            view_state.expanded_epic_ids.remove(&task_id);
            view_state.collapsed_epic_ids.insert(task_id.clone());
            format!("collapsed epic {display_ref}")
        } else {
            view_state.collapsed_epic_ids.remove(&task_id);
            view_state.expanded_epic_ids.insert(task_id.clone());
            format!("expanded epic {display_ref}")
        };
        self.refresh_with_view_state(view_state, Some(&task_id))
            .await?;
        let selected = self.tasks.iter().position(|task| task.task.id == task_id);
        Ok(Some(MutationMessage::new(message, selected)))
    }

    pub(crate) fn resolve_add_epic_child_context(
        &self,
        selected: Option<usize>,
    ) -> Option<AddEpicChildContext> {
        if let Some(epic) = self.resolve_epic_context(selected) {
            return Some(AddEpicChildContext::Existing(epic));
        }
        let item = self.selected_task(selected)?;
        Some(AddEpicChildContext::Promote(EpicContext {
            epic_id: item.task.id.clone(),
            display_ref: item.display_ref.clone(),
            project_key: item.task.project_key.clone(),
        }))
    }

    fn resolve_epic_context(&self, selected: Option<usize>) -> Option<EpicContext> {
        let item = self.selected_task(selected)?;
        if item.task.is_epic {
            return Some(EpicContext {
                epic_id: item.task.id.clone(),
                display_ref: item.display_ref.clone(),
                project_key: item.task.project_key.clone(),
            });
        }
        if let Some(parent) = &item.epic_parent {
            return Some(EpicContext {
                epic_id: parent.task_id.clone(),
                display_ref: parent.display_ref.clone(),
                project_key: item.task.project_key.clone(),
            });
        }
        self.find_epic_child_target(selected)
            .map(|target| target.epic)
    }

    pub(crate) fn resolve_epic_child_target(
        &self,
        selected: Option<usize>,
        focused_child_id: Option<&crate::ids::TaskId>,
    ) -> Option<EpicChildTarget> {
        if let Some(child_id) = focused_child_id {
            let parent = self.selected_task(selected)?;
            let (original_position, child) = parent
                .epic_children
                .iter()
                .enumerate()
                .find(|(_, child)| &child.task_id == child_id)?;
            return Some(EpicChildTarget {
                epic: EpicContext {
                    epic_id: parent.task.id.clone(),
                    display_ref: parent.display_ref.clone(),
                    project_key: parent.task.project_key.clone(),
                },
                child: child.clone(),
                original_position,
            });
        }
        if let Some(target) = self.find_epic_child_target(selected) {
            return Some(target);
        }
        let item = self.selected_task(selected)?;
        let parent = item.epic_parent.as_ref()?;
        Some(EpicChildTarget {
            epic: EpicContext {
                epic_id: parent.task_id.clone(),
                display_ref: parent.display_ref.clone(),
                project_key: item.task.project_key.clone(),
            },
            child: TaskDependencyLink {
                task_id: item.task.id.clone(),
                display_ref: item.display_ref.clone(),
                title: item.task.title.clone(),
                status: item.task.status.as_str().to_string(),
                priority: item.task.priority.as_str().to_string(),
                unresolved: !matches!(item.task.status.as_str(), "done" | "canceled"),
            },
            original_position: 0,
        })
    }

    pub(crate) async fn add_epic_child(
        &mut self,
        epic: EpicContext,
        child_id: crate::ids::TaskId,
    ) -> Result<EpicChildMutation, anyhow::Error> {
        let display_refs = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?;
        let child = self
            .database
            .resolve_task_ref(&self.active_workspace, child_id.as_str())
            .await?;
        let child_ref = display_refs.display_ref(&child);
        let outcome = self
            .database
            .add_task_to_epic_with_undo(
                &self.active_workspace,
                &child_id,
                &epic.epic_id,
                UndoContext::tui(format!("add {child_ref} to {}", epic.display_ref)),
            )
            .await?;
        self.wake_after_mutation();
        let mut view_state = self.view_state.clone();
        if view_state.render_mode() == TaskListRenderMode::Epics {
            view_state.collapsed_epic_ids.remove(&epic.epic_id);
            view_state.expanded_epic_ids.insert(epic.epic_id.clone());
        }
        self.refresh_with_view_state(view_state, Some(&epic.epic_id))
            .await?;
        let selected = self.tasks.iter().position(|item| item.task.id == child_id);
        let live_child = self
            .tasks
            .iter()
            .find(|item| item.task.id == epic.epic_id)
            .and_then(|item| {
                item.epic_children
                    .iter()
                    .enumerate()
                    .find(|(_, child)| child.task_id == child_id)
            });
        let original_position = live_child.map(|(position, _)| position).unwrap_or(0);
        let child = live_child
            .map(|(_, child)| child.clone())
            .unwrap_or(TaskDependencyLink {
                task_id: outcome.child.id,
                display_ref: child_ref.clone(),
                title: outcome.child.title,
                status: outcome.child.status.as_str().to_string(),
                priority: outcome.child.priority.as_str().to_string(),
                unresolved: !matches!(outcome.child.status.as_str(), "done" | "canceled"),
            });
        let message = if outcome.changed {
            format!("Added {child_ref} to {}", epic.display_ref)
        } else {
            format!("{child_ref} is already a child of {}", epic.display_ref)
        };
        Ok(EpicChildMutation {
            message: MutationMessage::new(message, selected),
            epic,
            child,
            original_position,
            changed: outcome.changed,
        })
    }

    pub(crate) async fn remove_epic_child(
        &mut self,
        target: EpicChildTarget,
    ) -> Result<EpicChildMutation, anyhow::Error> {
        let outcome = self
            .database
            .remove_task_from_epic_with_undo(
                &self.active_workspace,
                &target.child.task_id,
                &target.epic.epic_id,
                UndoContext::tui(format!(
                    "remove {} from {}",
                    target.child.display_ref, target.epic.display_ref
                )),
            )
            .await?;
        self.wake_after_mutation();
        self.refresh(Some(&target.epic.epic_id)).await?;
        let selected = self
            .tasks
            .iter()
            .position(|item| item.task.id == target.epic.epic_id);
        let message = if outcome.changed {
            format!(
                "Removed {} from {}",
                target.child.display_ref, target.epic.display_ref
            )
        } else {
            format!(
                "{} is already removed from {}",
                target.child.display_ref, target.epic.display_ref
            )
        };
        Ok(EpicChildMutation {
            message: MutationMessage::new(message, selected),
            epic: target.epic,
            child: target.child,
            original_position: target.original_position,
            changed: outcome.changed,
        })
    }

    fn find_epic_child_target(&self, selected: Option<usize>) -> Option<EpicChildTarget> {
        if self.view_state.render_mode() != TaskListRenderMode::Epics {
            return None;
        }
        let selected_task_id = self.tasks.get(selected?)?.task.id.clone();
        for item in &self.tasks {
            if !self.view_state.expanded_epic_ids.contains(&item.task.id) {
                continue;
            }
            if let Some((original_position, child)) = item
                .epic_children
                .iter()
                .enumerate()
                .find(|(_, child)| child.task_id == selected_task_id)
            {
                return Some(EpicChildTarget {
                    epic: EpicContext {
                        epic_id: item.task.id.clone(),
                        display_ref: item.display_ref.clone(),
                        project_key: item.task.project_key.clone(),
                    },
                    child: child.clone(),
                    original_position,
                });
            }
        }
        None
    }
}

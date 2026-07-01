use crate::operations::remove_task_from_epic;
use crate::tui::store::MutationMessage;

use super::{TaskListRenderMode, TuiStore};

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
        let message = if self.view_state.expanded_epic_ids.contains(&task_id) {
            self.view_state.expanded_epic_ids.remove(&task_id);
            self.view_state.collapsed_epic_ids.insert(task_id.clone());
            format!("collapsed epic {}", display_ref)
        } else {
            self.view_state.collapsed_epic_ids.remove(&task_id);
            self.view_state.expanded_epic_ids.insert(task_id.clone());
            format!("expanded epic {}", display_ref)
        };
        self.refresh(Some(&task_id)).await?;
        let selected = self.tasks.iter().position(|task| task.task.id == task_id);
        Ok(Some(MutationMessage::new(message, selected)))
    }

    pub(crate) async fn detach_selected_epic_child(
        &mut self,
        index: Option<usize>,
    ) -> Result<Option<MutationMessage>, anyhow::Error> {
        let Some((child_id, parent_id)) = self.find_epic_child_pair(index) else {
            return Ok(None);
        };
        let child_display_ref = self
            .tasks
            .iter()
            .find(|t| t.task.id == child_id)
            .map(|t| t.display_ref.clone())
            .unwrap_or_default();
        let parent_display_ref = self
            .tasks
            .iter()
            .find(|t| t.task.id == parent_id)
            .map(|t| t.display_ref.clone())
            .unwrap_or_default();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        remove_task_from_epic(&mut conn, &child_id, &parent_id).await?;
        drop(conn);
        let message = format!("detached {} from {}", child_display_ref, parent_display_ref);
        self.refresh(None).await?;
        let selected = self.tasks.iter().position(|t| t.task.id == parent_id);
        Ok(Some(MutationMessage::new(message, selected)))
    }

    pub(crate) async fn promote_selected_epic_child(
        &mut self,
        index: Option<usize>,
    ) -> Result<Option<MutationMessage>, anyhow::Error> {
        let Some((child_id, parent_id)) = self.find_epic_child_pair(index) else {
            return Ok(None);
        };
        let child_display_ref = self
            .tasks
            .iter()
            .find(|t| t.task.id == child_id)
            .map(|t| t.display_ref.clone())
            .unwrap_or_default();
        let parent_display_ref = self
            .tasks
            .iter()
            .find(|t| t.task.id == parent_id)
            .map(|t| t.display_ref.clone())
            .unwrap_or_default();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        remove_task_from_epic(&mut conn, &child_id, &parent_id).await?;
        drop(conn);
        let message = format!("promoted {} from {}", child_display_ref, parent_display_ref);
        self.refresh(None).await?;
        let selected = self.tasks.iter().position(|t| t.task.id == child_id);
        Ok(Some(MutationMessage::new(message, selected)))
    }

    fn find_epic_child_pair(&self, selected: Option<usize>) -> Option<(String, String)> {
        if self.view_state.render_mode() != TaskListRenderMode::Epics {
            return None;
        }
        let selected_task_id = self
            .tasks
            .get(selected?)
            .map(|task| task.task.id.as_str())?;
        for item in &self.tasks {
            if !self.view_state.expanded_epic_ids.contains(&item.task.id) {
                continue;
            }
            for link in &item.epic_children {
                if link.unresolved && link.task_id == selected_task_id {
                    return Some((link.task_id.clone(), item.task.id.clone()));
                }
            }
        }
        None
    }
}

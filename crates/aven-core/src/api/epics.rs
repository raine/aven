use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosEpicProgress {
    pub total: u32,
    pub open: u32,
    pub done: u32,
    pub canceled: u32,
}

impl From<crate::query::EpicRollup> for IosEpicProgress {
    fn from(value: crate::query::EpicRollup) -> Self {
        Self {
            total: value.total.min(u32::MAX as usize) as u32,
            open: value.open.min(u32::MAX as usize) as u32,
            done: value.done.min(u32::MAX as usize) as u32,
            canceled: value.canceled.min(u32::MAX as usize) as u32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosEpicSummary {
    pub task: IosTaskListRow,
    pub progress: IosEpicProgress,
}

impl Store {
    pub async fn ios_epics(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<IosEpicSummary>, Error> {
        self.workspace(workspace_id).await?;
        let items = self
            .database
            .list_task_summary_items(
                workspace_id,
                TaskFilters {
                    epics_only: true,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Updated,
                SortDirection::Desc,
                None,
            )
            .await
            .map_err(Error::from_internal)?;
        Ok(items
            .into_iter()
            .map(|mut item| IosEpicSummary {
                progress: item.epic_rollup.take().unwrap_or_default().into(),
                task: item.into(),
            })
            .collect())
    }

    pub async fn ios_epic_candidates(
        &self,
        workspace_id: &WorkspaceId,
        epic_id: &TaskId,
    ) -> Result<Vec<IosTaskListRow>, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let epic = self
            .database
            .resolve_task_ref(&workspace, epic_id.as_str())
            .await
            .map_err(Error::from_internal)?;
        if epic.deleted || !epic.is_epic {
            return Err(Error::new(
                ErrorCode::Validation,
                "epic is unavailable".into(),
            ));
        }
        self.database
            .list_task_summary_items(
                workspace_id,
                TaskFilters {
                    project: Some(epic.project_key),
                    exclude_epics: true,
                    without_epic_link: true,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Updated,
                SortDirection::Desc,
                None,
            )
            .await
            .map(|items| items.into_iter().map(Into::into).collect())
            .map_err(Error::from_internal)
    }

    pub async fn create_ios_epic(
        &self,
        workspace_id: &WorkspaceId,
        input: IosTaskCapture,
    ) -> Result<TaskId, Error> {
        self.create_ios_epic_task(workspace_id, None, input).await
    }

    pub async fn create_ios_epic_child(
        &self,
        workspace_id: &WorkspaceId,
        epic_id: &TaskId,
        input: IosTaskCapture,
    ) -> Result<TaskId, Error> {
        self.create_ios_epic_task(workspace_id, Some(epic_id.clone()), input)
            .await
    }

    async fn create_ios_epic_task(
        &self,
        workspace_id: &WorkspaceId,
        epic_id: Option<TaskId>,
        input: IosTaskCapture,
    ) -> Result<TaskId, Error> {
        validate_optional_date("due_on", input.due_on.as_deref())?;
        let workspace = self.workspace(workspace_id).await?;
        let project = input
            .project
            .ok_or_else(|| Error::new(ErrorCode::Validation, "project is required".into()))?;
        let outcome = self
            .database
            .create_task_with_options(
                &workspace,
                TaskDraft {
                    title: input.title.trim().to_string(),
                    description: input.description,
                    project: Some(project),
                    status: TaskStatus::Inbox.as_str().into(),
                    priority: input.priority.as_str().into(),
                    source: TaskSource::Api,
                    labels: input.labels,
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: input.due_on,
                    is_epic: epic_id.is_none(),
                },
                TaskCreationOptions::for_ios_epic(epic_id),
            )
            .await
            .map_err(Error::from_internal)?;
        Ok(outcome.task.id)
    }

    pub async fn set_ios_epic_child(
        &self,
        workspace_id: &WorkspaceId,
        epic_id: &TaskId,
        child_id: &TaskId,
        linked: bool,
    ) -> Result<bool, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .set_ios_epic_child(&workspace, epic_id, child_id, linked)
            .await
            .map_err(Error::from_internal)
    }
}

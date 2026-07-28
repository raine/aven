use crate::attachments::storage::sha256_hex;
use crate::operations::{AttachmentAddInput, TaskDraft};
use crate::tui::overlay::SearchState;
use crate::tui::store::EpicContext;

pub(crate) const ADD_NOTE_TITLE: &str = "Add note";
pub(crate) const ADD_TASK_TITLE_PROJECT_TITLE: &str = "Add task: project";
pub(crate) const ADD_TASK_LABELS_TITLE: &str = "Add task: labels";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddTaskStep {
    Project,
    Status,
    Priority,
    Labels,
    Title,
    AvailableAt,
    Due,
    Images,
    Description,
}

impl AddTaskStep {
    pub(crate) const ALL: [Self; 9] = [
        Self::Project,
        Self::Status,
        Self::Priority,
        Self::Labels,
        Self::AvailableAt,
        Self::Due,
        Self::Images,
        Self::Title,
        Self::Description,
    ];

    pub(crate) fn next(self, reverse: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|field| *field == self)
            .unwrap_or(0);
        let next = if reverse {
            index.checked_sub(1).unwrap_or(Self::ALL.len() - 1)
        } else {
            (index + 1) % Self::ALL.len()
        };
        Self::ALL[next]
    }

    pub(crate) fn is_metadata(self) -> bool {
        matches!(
            self,
            Self::Project
                | Self::Status
                | Self::Priority
                | Self::Labels
                | Self::AvailableAt
                | Self::Due
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingTaskAttachmentSummary {
    pub(crate) filename: String,
    pub(crate) byte_size: i64,
    pub(crate) dimensions: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingTaskAttachment {
    pub(crate) attachment_id: String,
    pub(crate) sha256: String,
    pub(crate) input: AttachmentAddInput,
}

impl PendingTaskAttachment {
    pub(crate) fn new(attachment_id: String, input: AttachmentAddInput) -> Self {
        let sha256 = sha256_hex(&input.bytes);
        Self {
            attachment_id,
            sha256,
            input,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddTaskOrigin {
    Standalone,
    EpicChild {
        epic: EpicContext,
        return_search: Box<SearchState>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AddTaskDraftState {
    origin: AddTaskOrigin,
    title: String,
    description: String,
    project: Option<String>,
    inferred_project: Option<String>,
    status: String,
    priority: String,
    labels: Vec<String>,
    available_at: String,
    due_on: String,
    attachments: Vec<PendingTaskAttachment>,
    step: AddTaskStep,
}

impl Default for AddTaskDraftState {
    fn default() -> Self {
        Self {
            origin: AddTaskOrigin::Standalone,
            title: String::new(),
            description: String::new(),
            project: None,
            inferred_project: None,
            status: "inbox".to_string(),
            priority: "none".to_string(),
            labels: Vec::new(),
            available_at: String::new(),
            due_on: String::new(),
            attachments: Vec::new(),
            step: AddTaskStep::Title,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct AuthoringState {
    flow: Option<AddTaskDraftState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddTaskContext {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) step: AddTaskStep,
    pub(crate) project: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) available_at: String,
    pub(crate) due_on: String,
}

#[derive(Debug, Clone)]
pub(crate) struct AddTaskSubmissionContext {
    pub(crate) origin: AddTaskOrigin,
    pub(crate) attachments: Vec<PendingTaskAttachment>,
}

#[cfg(test)]
pub(crate) struct AddTaskCreate {
    pub(crate) draft: TaskDraft,
    pub(crate) attachments: Vec<PendingTaskAttachment>,
}

#[cfg(test)]
pub(crate) enum AddTaskTitleSubmit {
    Create(Box<AddTaskCreate>),
    ReopenTitle { message: &'static str },
    Inactive,
}

impl AuthoringState {
    pub(crate) fn begin_add_task(
        &mut self,
        active_project: Option<String>,
        inferred_project: Option<String>,
    ) {
        self.flow = Some(AddTaskDraftState {
            origin: AddTaskOrigin::Standalone,
            project: active_project,
            inferred_project,
            ..AddTaskDraftState::default()
        });
    }

    pub(crate) fn begin_epic_child(
        &mut self,
        epic: EpicContext,
        return_search: SearchState,
        title: String,
    ) {
        self.flow = Some(AddTaskDraftState {
            origin: AddTaskOrigin::EpicChild {
                epic: epic.clone(),
                return_search: Box::new(return_search),
            },
            title,
            project: Some(epic.project_key),
            ..AddTaskDraftState::default()
        });
    }

    pub(crate) fn is_standalone_add_task(&self) -> bool {
        matches!(
            self.flow.as_ref().map(|flow| &flow.origin),
            Some(AddTaskOrigin::Standalone)
        )
    }

    pub(crate) fn submission_context(&self) -> Option<AddTaskSubmissionContext> {
        let flow = self.flow.as_ref()?;
        Some(AddTaskSubmissionContext {
            origin: flow.origin.clone(),
            attachments: flow.attachments.clone(),
        })
    }

    pub(crate) fn add_task_context(&self) -> Option<AddTaskContext> {
        let draft = self.flow.as_ref()?;
        let project = draft
            .project
            .as_deref()
            .or(draft.inferred_project.as_deref())
            .unwrap_or("no project");
        Some(AddTaskContext {
            title: draft.title.clone(),
            description: draft.description.clone(),
            step: draft.step,
            project: project.to_string(),
            status: draft.status.clone(),
            priority: draft.priority.clone(),
            labels: draft.labels.clone(),
            available_at: draft.available_at.clone(),
            due_on: draft.due_on.clone(),
        })
    }

    pub(crate) fn selected_add_task_project(&self) -> Option<Option<String>> {
        let draft = self.flow.as_ref()?;
        Some(draft.project.clone())
    }

    pub(crate) fn apply_add_task_status(&mut self, status: &str) -> Option<String> {
        let draft = self.flow.as_mut()?;
        if !crate::choices::STATUSES.contains(&status) {
            return None;
        }
        draft.status = status.to_string();
        Some(draft.status.clone())
    }

    pub(crate) fn add_pending_add_task_attachment(
        &mut self,
        attachment: PendingTaskAttachment,
    ) -> Option<bool> {
        let draft = self.flow.as_mut()?;
        if draft
            .attachments
            .iter()
            .any(|existing| existing.sha256 == attachment.sha256)
        {
            return Some(false);
        }
        draft.attachments.push(attachment);
        Some(true)
    }

    pub(crate) fn add_task_has_pending_attachments(&self) -> bool {
        matches!(self.flow.as_ref(), Some(draft) if !draft.attachments.is_empty())
    }

    #[cfg(test)]
    pub(crate) fn add_task_attachments(&self) -> Vec<PendingTaskAttachment> {
        let Some(draft) = self.flow.as_ref() else {
            return Vec::new();
        };
        draft.attachments.clone()
    }

    pub(crate) fn add_task_attachment_summaries(&self) -> Vec<PendingTaskAttachmentSummary> {
        let Some(draft) = self.flow.as_ref() else {
            return Vec::new();
        };
        draft
            .attachments
            .iter()
            .map(|attachment| PendingTaskAttachmentSummary {
                filename: pending_attachment_filename(attachment),
                byte_size: attachment.input.bytes.len() as i64,
                dimensions: image::ImageReader::new(std::io::Cursor::new(
                    attachment.input.bytes.as_slice(),
                ))
                .with_guessed_format()
                .ok()
                .and_then(|reader| reader.into_dimensions().ok()),
            })
            .collect()
    }

    pub(crate) fn remove_add_task_attachment(&mut self, index: usize) -> Option<String> {
        let draft = self.flow.as_mut()?;
        if index >= draft.attachments.len() {
            return None;
        }
        let attachment = draft.attachments.remove(index);
        Some(
            attachment
                .input
                .filename
                .unwrap_or_else(|| "pasted image".to_string()),
        )
    }

    pub(crate) fn capture_add_task_fields(
        &mut self,
        title: String,
        description: String,
        step: AddTaskStep,
    ) -> bool {
        let Some(draft) = self.flow.as_mut() else {
            return false;
        };
        draft.title = title;
        draft.description = description;
        draft.step = step;
        true
    }

    pub(crate) fn apply_add_task_project(&mut self, values: Vec<String>) -> bool {
        let Some(draft) = self.flow.as_mut() else {
            return false;
        };
        draft.project = values.first().filter(|value| !value.is_empty()).cloned();
        true
    }

    pub(crate) fn apply_add_task_priority(&mut self, values: Vec<String>) -> bool {
        let Some(draft) = self.flow.as_mut() else {
            return false;
        };
        draft.priority = values
            .first()
            .cloned()
            .unwrap_or_else(|| "none".to_string());
        true
    }

    pub(crate) fn apply_add_task_labels(&mut self, values: Vec<String>) -> bool {
        let Some(draft) = self.flow.as_mut() else {
            return false;
        };
        draft.labels = values;
        true
    }

    pub(crate) fn apply_add_task_available_at(&mut self, value: String) -> bool {
        let Some(draft) = self.flow.as_mut() else {
            return false;
        };
        draft.available_at = value;
        true
    }

    pub(crate) fn apply_add_task_due_on(&mut self, value: String) -> bool {
        let Some(draft) = self.flow.as_mut() else {
            return false;
        };
        draft.due_on = value;
        true
    }

    pub(crate) fn apply_add_task_priority_value(&mut self, priority: &str) -> Option<String> {
        let draft = self.flow.as_mut()?;
        if !crate::choices::PRIORITIES.contains(&priority) {
            return None;
        }
        draft.priority = priority.to_string();
        Some(draft.priority.clone())
    }

    pub(crate) fn apply_add_task_draft(&mut self, task: TaskDraft) -> bool {
        let Some(draft) = self.flow.as_mut() else {
            return false;
        };
        draft.title = task.title;
        draft.description = task.description;
        draft.project = task.project;
        draft.status = task.status;
        draft.priority = task.priority;
        draft.labels = task.labels;
        draft.available_at = task.available_at.unwrap_or_default();
        draft.due_on = task.due_on.unwrap_or_default();
        draft.step = AddTaskStep::Title;
        true
    }

    #[cfg(test)]
    pub(crate) fn submit_add_task(&mut self) -> AddTaskTitleSubmit {
        let Some(draft) = self.flow.take() else {
            return AddTaskTitleSubmit::Inactive;
        };
        let trimmed = draft.title.trim();
        if trimmed.is_empty() {
            self.flow = Some(draft);
            return AddTaskTitleSubmit::ReopenTitle {
                message: "task title is required",
            };
        }
        let description = draft.description.trim().to_string();
        AddTaskTitleSubmit::Create(Box::new(AddTaskCreate {
            draft: TaskDraft {
                title: trimmed.to_string(),
                description,
                project: draft.project,
                status: draft.status,
                priority: draft.priority,
                source: crate::choices::TaskSource::Tui,
                labels: draft.labels,
                available_at: (!draft.available_at.is_empty()).then_some(draft.available_at),
                due_on: (!draft.due_on.is_empty()).then_some(draft.due_on),
                is_epic: false,
            },
            attachments: draft.attachments,
        }))
    }

    pub(crate) fn cancel_add_task(&mut self) -> Option<AddTaskOrigin> {
        self.flow.take().map(|flow| flow.origin)
    }

    pub(crate) fn clear(&mut self) {
        self.flow = None;
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.flow.is_none()
    }
}

fn pending_attachment_filename(attachment: &PendingTaskAttachment) -> String {
    attachment
        .input
        .filename
        .clone()
        .unwrap_or_else(|| "pasted image".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::optimization::ImageOptimizationPolicy;

    #[test]
    fn add_task_project_selection_retains_flow() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, Some("mobile-app".to_string()));
        assert!(state.capture_add_task_fields(
            "Write docs".to_string(),
            String::new(),
            AddTaskStep::Title,
        ));
        assert!(state.apply_add_task_project(vec!["mobile-app".to_string()]));
        assert!(state.add_task_context().is_some());
    }

    #[test]
    fn add_task_status_defaults_to_inbox_and_can_be_set() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);

        assert_eq!(state.add_task_context().unwrap().status, "inbox");
        assert_eq!(state.apply_add_task_status("todo").as_deref(), Some("todo"));
        assert!(matches!(
            state.submit_add_task(),
            AddTaskTitleSubmit::ReopenTitle { .. }
        ));
        assert_eq!(state.add_task_context().unwrap().status, "todo");
    }

    #[test]
    fn add_task_description_is_stored_in_created_draft() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);
        assert!(state.capture_add_task_fields(
            "Write docs".to_string(),
            "Details\nfor handoff".to_string(),
            AddTaskStep::Description,
        ));
        assert!(matches!(
            state.submit_add_task(),
            AddTaskTitleSubmit::Create(create)
                if create.draft.description == "Details\nfor handoff"
        ));
    }

    #[test]
    fn add_task_labels_are_stored_in_created_draft() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);
        assert!(state.capture_add_task_fields(
            "Write docs".to_string(),
            String::new(),
            AddTaskStep::Title,
        ));
        assert!(state.apply_add_task_labels(vec!["feature".to_string(), "ui".to_string()]));
        assert!(matches!(
            state.submit_add_task(),
            AddTaskTitleSubmit::Create(create)
                if create.draft.labels == vec!["feature".to_string(), "ui".to_string()]
        ));
    }

    #[test]
    fn add_task_description_context_marks_active_step() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);
        assert!(state.capture_add_task_fields(
            "Write docs".to_string(),
            "Details".to_string(),
            AddTaskStep::Description,
        ));
        let context = state.add_task_context().unwrap();
        assert_eq!(context.step, AddTaskStep::Description);
        assert_eq!(context.title, "Write docs");
        assert_eq!(context.description, "Details");
    }

    #[test]
    fn blank_title_reopens_without_consuming_add_task() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);
        assert!(matches!(
            state.submit_add_task(),
            AddTaskTitleSubmit::ReopenTitle {
                message: "task title is required"
            }
        ));
        assert!(state.add_task_context().is_some());
    }

    #[test]
    fn project_empty_value_keeps_project_none_for_inferred_create() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, Some("mobile-app".to_string()));
        assert!(state.capture_add_task_fields(
            "Write docs".to_string(),
            String::new(),
            AddTaskStep::Title,
        ));
        assert!(state.apply_add_task_project(vec![String::new()]));
        assert!(matches!(
            state.submit_add_task(),
            AddTaskTitleSubmit::Create(create) if create.draft.project.is_none()
        ));
    }

    #[test]
    fn pending_attachment_survives_parsed_draft_without_entering_description() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);
        let pending = PendingTaskAttachment::new(
            "ATTACHMENT000001".to_string(),
            AttachmentAddInput {
                filename: Some("chart.png".to_string()),
                alt_text: Some("Chart".to_string()),
                declared_media_type: Some("image/png".to_string()),
                bytes: vec![1, 2, 3],
                optimization_policy: ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        );
        assert_eq!(state.add_pending_add_task_attachment(pending), Some(true));
        assert!(state.apply_add_task_draft(TaskDraft {
            title: "Parsed".to_string(),
            description: "User-authored details".to_string(),
            project: None,
            status: "inbox".to_string(),
            priority: "none".to_string(),
            source: crate::choices::TaskSource::Tui,
            labels: Vec::new(),
            available_at: None,
            due_on: None,
            is_epic: false,
        }));

        assert_eq!(
            state.add_task_context().unwrap().description,
            "User-authored details"
        );
        assert_eq!(state.add_task_attachments().len(), 1);
        let AddTaskTitleSubmit::Create(create) = state.submit_add_task() else {
            panic!("pending attachment task should be ready to create");
        };
        assert_eq!(create.attachments.len(), 1);
    }

    #[test]
    fn standalone_origin_is_part_of_active_flow() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);

        assert!(matches!(
            state.submission_context().map(|context| context.origin),
            Some(AddTaskOrigin::Standalone)
        ));
    }

    #[test]
    fn epic_child_origin_owns_epic_and_search_return() {
        let mut state = AuthoringState::default();
        let epic = EpicContext {
            epic_id: crate::ids::TaskId::new(),
            display_ref: "APP-EPIC".to_string(),
            project_key: "app".to_string(),
        };
        let mut search = SearchState::for_intent(crate::tui::overlay::SearchIntent::AddEpicChild {
            epic_id: epic.epic_id.clone(),
            display_ref: epic.display_ref.clone(),
            project_key: epic.project_key.clone(),
        });
        search.input = crate::tui::overlay::LineEdit::new("Child title".to_string());

        state.begin_epic_child(epic.clone(), search.clone(), "Child title".to_string());

        let context = state.submission_context().unwrap();
        assert!(matches!(
            context.origin,
            AddTaskOrigin::EpicChild {
                epic: owned_epic,
                return_search,
            } if owned_epic == epic && *return_search == search
        ));
        let origin = state.cancel_add_task().unwrap();
        assert!(matches!(origin, AddTaskOrigin::EpicChild { .. }));
        assert!(state.is_idle());
    }

    #[test]
    fn cancel_discards_pending_attachments() {
        let mut state = AuthoringState::default();
        state.begin_add_task(None, None);
        let pending = PendingTaskAttachment::new(
            "ATTACHMENT000001".to_string(),
            AttachmentAddInput {
                filename: Some("chart.png".to_string()),
                alt_text: None,
                declared_media_type: Some("image/png".to_string()),
                bytes: vec![1],
                optimization_policy: ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        );
        assert_eq!(state.add_pending_add_task_attachment(pending), Some(true));
        state.cancel_add_task();
        state.begin_add_task(None, None);

        assert!(!state.add_task_has_pending_attachments());
    }
}

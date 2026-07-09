use crate::attachments::storage::sha256_hex;
use crate::operations::{
    AttachmentAddInput, TaskDraft, append_attachment_ref, markdown_attachment_ref,
};

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
    Description,
}

impl AddTaskStep {
    pub(crate) const ALL: [Self; 8] = [
        Self::Project,
        Self::Status,
        Self::Priority,
        Self::Labels,
        Self::AvailableAt,
        Self::Due,
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

    pub(crate) fn markdown_ref(&self) -> String {
        markdown_attachment_ref(&self.attachment_id, self.input.alt_text.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AddTaskDraftState {
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthoringFlow {
    AddTask(AddTaskDraftState),
    AddNote {
        task_id: crate::ids::TaskId,
        display_ref: String,
        return_to_detail: bool,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct AuthoringState {
    flow: Option<AuthoringFlow>,
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

#[cfg(test)]
pub(crate) struct AddTaskCreate {
    pub(crate) draft: TaskDraft,
    pub(crate) attachments: Vec<PendingTaskAttachment>,
}

#[cfg(test)]
pub(crate) enum AddTaskTitleSubmit {
    Create(AddTaskCreate),
    ReopenTitle { message: &'static str },
    Inactive,
}

pub(crate) enum AddNoteSubmit {
    Create {
        task_id: crate::ids::TaskId,
        display_ref: String,
        body: String,
        return_to_detail: bool,
    },
    Blank {
        return_to_detail: bool,
        message: &'static str,
    },
    Inactive {
        message: &'static str,
    },
}

impl AuthoringState {
    pub(crate) fn begin_add_task(
        &mut self,
        active_project: Option<String>,
        inferred_project: Option<String>,
    ) {
        self.flow = Some(AuthoringFlow::AddTask(AddTaskDraftState {
            project: active_project,
            inferred_project,
            ..AddTaskDraftState::default()
        }));
    }

    pub(crate) fn begin_add_note(
        &mut self,
        task_id: crate::ids::TaskId,
        display_ref: String,
        return_to_detail: bool,
    ) {
        self.flow = Some(AuthoringFlow::AddNote {
            task_id,
            display_ref,
            return_to_detail,
        });
    }

    pub(crate) fn add_task_context(&self) -> Option<AddTaskContext> {
        let AuthoringFlow::AddTask(draft) = self.flow.as_ref()? else {
            return None;
        };
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
        let AuthoringFlow::AddTask(draft) = self.flow.as_ref()? else {
            return None;
        };
        Some(draft.project.clone())
    }

    pub(crate) fn apply_add_task_status(&mut self, status: &str) -> Option<String> {
        let AuthoringFlow::AddTask(draft) = self.flow.as_mut()? else {
            return None;
        };
        if !crate::choices::STATUSES.contains(&status) {
            return None;
        }
        draft.status = status.to_string();
        Some(draft.status.clone())
    }

    pub(crate) fn add_pending_add_task_attachment(
        &mut self,
        attachment: PendingTaskAttachment,
    ) -> Option<(String, bool)> {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return None;
        };
        if let Some(existing) = draft
            .attachments
            .iter()
            .find(|existing| existing.sha256 == attachment.sha256)
        {
            return Some((existing.markdown_ref(), false));
        }
        let ref_text = attachment.markdown_ref();
        draft.attachments.push(attachment);
        Some((ref_text, true))
    }

    pub(crate) fn add_task_attachment_refs(&self) -> Vec<String> {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_ref() else {
            return Vec::new();
        };
        draft
            .attachments
            .iter()
            .map(PendingTaskAttachment::markdown_ref)
            .collect()
    }

    pub(crate) fn add_task_has_pending_attachments(&self) -> bool {
        matches!(self.flow.as_ref(), Some(AuthoringFlow::AddTask(draft)) if !draft.attachments.is_empty())
    }

    pub(crate) fn take_add_task_attachments(&mut self) -> Vec<PendingTaskAttachment> {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return Vec::new();
        };
        std::mem::take(&mut draft.attachments)
    }

    pub(crate) fn clear_add_task(&mut self) {
        if matches!(self.flow, Some(AuthoringFlow::AddTask(_))) {
            self.flow = None;
        }
    }

    pub(crate) fn append_missing_add_task_attachment_refs(&self, description: &str) -> String {
        append_missing_refs(description, self.add_task_attachment_refs())
    }

    pub(crate) fn capture_add_task_fields(
        &mut self,
        title: String,
        description: String,
        step: AddTaskStep,
    ) -> bool {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return false;
        };
        draft.title = title;
        draft.description = description;
        draft.step = step;
        true
    }

    pub(crate) fn apply_add_task_project(&mut self, values: Vec<String>) -> bool {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return false;
        };
        draft.project = values.first().filter(|value| !value.is_empty()).cloned();
        true
    }

    pub(crate) fn apply_add_task_priority(&mut self, values: Vec<String>) -> bool {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return false;
        };
        draft.priority = values
            .first()
            .cloned()
            .unwrap_or_else(|| "none".to_string());
        true
    }

    pub(crate) fn apply_add_task_labels(&mut self, values: Vec<String>) -> bool {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return false;
        };
        draft.labels = values;
        true
    }

    pub(crate) fn apply_add_task_available_at(&mut self, value: String) -> bool {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return false;
        };
        draft.available_at = value;
        true
    }

    pub(crate) fn apply_add_task_due_on(&mut self, value: String) -> bool {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return false;
        };
        draft.due_on = value;
        true
    }

    pub(crate) fn apply_add_task_priority_value(&mut self, priority: &str) -> Option<String> {
        let AuthoringFlow::AddTask(draft) = self.flow.as_mut()? else {
            return None;
        };
        if !crate::choices::PRIORITIES.contains(&priority) {
            return None;
        }
        draft.priority = priority.to_string();
        Some(draft.priority.clone())
    }

    pub(crate) fn apply_add_task_draft(&mut self, task: TaskDraft) -> bool {
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.as_mut() else {
            return false;
        };
        draft.title = task.title;
        draft.description = append_missing_refs(
            &task.description,
            draft
                .attachments
                .iter()
                .map(PendingTaskAttachment::markdown_ref),
        );
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
        let Some(AuthoringFlow::AddTask(draft)) = self.flow.take() else {
            return AddTaskTitleSubmit::Inactive;
        };
        let trimmed = draft.title.trim();
        if trimmed.is_empty() {
            self.flow = Some(AuthoringFlow::AddTask(draft));
            return AddTaskTitleSubmit::ReopenTitle {
                message: "task title is required",
            };
        }
        let description = append_missing_refs(
            draft.description.trim(),
            draft
                .attachments
                .iter()
                .map(PendingTaskAttachment::markdown_ref),
        );
        AddTaskTitleSubmit::Create(AddTaskCreate {
            draft: TaskDraft {
                title: trimmed.to_string(),
                description,
                project: draft.project,
                status: draft.status,
                priority: draft.priority,
                labels: draft.labels,
                available_at: (!draft.available_at.is_empty()).then_some(draft.available_at),
                due_on: (!draft.due_on.is_empty()).then_some(draft.due_on),
                is_epic: false,
            },
            attachments: draft.attachments,
        })
    }

    pub(crate) fn submit_add_note(&mut self, body: String) -> AddNoteSubmit {
        let Some(AuthoringFlow::AddNote {
            task_id,
            display_ref,
            return_to_detail,
        }) = self.flow.take()
        else {
            return AddNoteSubmit::Inactive {
                message: "no selected task for note",
            };
        };
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return AddNoteSubmit::Blank {
                return_to_detail,
                message: "note body is required",
            };
        }
        AddNoteSubmit::Create {
            task_id,
            display_ref,
            body: trimmed.to_string(),
            return_to_detail,
        }
    }

    pub(crate) fn cancel(&mut self) -> bool {
        let return_to_detail = matches!(
            self.flow,
            Some(AuthoringFlow::AddNote {
                return_to_detail: true,
                ..
            })
        );
        self.flow = None;
        return_to_detail
    }

    pub(crate) fn detail_underlay(&self) -> bool {
        matches!(
            self.flow,
            Some(AuthoringFlow::AddNote {
                return_to_detail: true,
                ..
            })
        )
    }

    pub(crate) fn clear(&mut self) {
        self.flow = None;
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.flow.is_none()
    }
}

fn append_missing_refs<I>(description: &str, refs: I) -> String
where
    I: IntoIterator<Item = String>,
{
    refs.into_iter()
        .fold(description.to_string(), |description, ref_text| {
            if description.contains(&ref_text) {
                description
            } else {
                append_attachment_ref(&description, &ref_text)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn add_note_blank_submit_consumes_flow_and_returns_detail_flag() {
        let mut state = AuthoringState::default();
        state.begin_add_note(
            crate::test_support::task_id("task-1"),
            "APP-1234".to_string(),
            true,
        );
        assert!(matches!(
            state.submit_add_note("   ".to_string()),
            AddNoteSubmit::Blank {
                return_to_detail: true,
                message: "note body is required"
            }
        ));
        assert!(state.is_idle());
    }
}

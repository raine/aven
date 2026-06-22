use crate::operations::TaskDraft;

pub(crate) const ADD_NOTE_TITLE: &str = "Add note";
pub(crate) const ADD_TASK_TITLE_PROJECT_TITLE: &str = "Add task: title project";
pub(crate) const ADD_TASK_TITLE_PRIORITY_TITLE: &str = "Add task: title priority";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddTaskStep {
    Title,
    Description,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AddTaskDraftState {
    title: String,
    description: String,
    project: Option<String>,
    inferred_project: Option<String>,
    priority: String,
    step: AddTaskStep,
}

impl Default for AddTaskDraftState {
    fn default() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            project: None,
            inferred_project: None,
            priority: "none".to_string(),
            step: AddTaskStep::Title,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthoringFlow {
    AddTask(AddTaskDraftState),
    AddNote {
        task_id: String,
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
    pub(crate) priority: String,
}

pub(crate) enum AddTaskTitleSubmit {
    Create(TaskDraft),
    ReopenTitle { message: &'static str },
    Inactive,
}

pub(crate) enum AddNoteSubmit {
    Create {
        task_id: String,
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
        task_id: String,
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
            priority: draft.priority.clone(),
        })
    }

    pub(crate) fn selected_add_task_project(&self) -> Option<Option<String>> {
        let AuthoringFlow::AddTask(draft) = self.flow.as_ref()? else {
            return None;
        };
        Some(draft.project.clone())
    }

    pub(crate) fn selected_add_task_priority(&self) -> Option<String> {
        let AuthoringFlow::AddTask(draft) = self.flow.as_ref()? else {
            return None;
        };
        Some(draft.priority.clone())
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
        AddTaskTitleSubmit::Create(TaskDraft {
            title: trimmed.to_string(),
            description: draft.description.trim().to_string(),
            project: draft.project,
            priority: draft.priority,
            labels: Vec::new(),
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
            AddTaskTitleSubmit::Create(draft)
                if draft.description == "Details\nfor handoff"
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
            AddTaskTitleSubmit::Create(draft) if draft.project.is_none()
        ));
    }

    #[test]
    fn add_note_blank_submit_consumes_flow_and_returns_detail_flag() {
        let mut state = AuthoringState::default();
        state.begin_add_note("task-1".to_string(), "APP-1234".to_string(), true);
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

use crate::tui::store::ConflictTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictResolutionChoice {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConflictFlow {
    PickVariant {
        choice: ConflictResolutionChoice,
        targets: Vec<ConflictTarget>,
    },
    ConfirmVariant {
        choice: ConflictResolutionChoice,
        target: ConflictTarget,
    },
    PickManual {
        targets: Vec<ConflictTarget>,
    },
    EditManual {
        target: ConflictTarget,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ConflictFlowState {
    flow: Option<ConflictFlow>,
}

pub(crate) enum ConflictTransition {
    PickField {
        targets: Vec<ConflictTarget>,
    },
    Confirm {
        choice: ConflictResolutionChoice,
        target: ConflictTarget,
    },
    EditManual {
        target: ConflictTarget,
    },
    Message(String),
}

pub(crate) enum ConflictSubmit {
    Resolve {
        target: ConflictTarget,
        value: String,
    },
    Inactive {
        message: &'static str,
    },
}

impl ConflictFlowState {
    pub(crate) fn begin_resolution(
        &mut self,
        choice: ConflictResolutionChoice,
        targets: Vec<ConflictTarget>,
    ) -> ConflictTransition {
        if targets.len() == 1 {
            let target = targets[0].clone();
            self.flow = Some(ConflictFlow::ConfirmVariant {
                choice,
                target: target.clone(),
            });
            ConflictTransition::Confirm { choice, target }
        } else {
            self.flow = Some(ConflictFlow::PickVariant {
                choice,
                targets: targets.clone(),
            });
            ConflictTransition::PickField { targets }
        }
    }

    pub(crate) fn begin_manual(&mut self, targets: Vec<ConflictTarget>) -> ConflictTransition {
        if targets.len() == 1 {
            let target = targets[0].clone();
            self.flow = Some(ConflictFlow::EditManual {
                target: target.clone(),
            });
            ConflictTransition::EditManual { target }
        } else {
            self.flow = Some(ConflictFlow::PickManual {
                targets: targets.clone(),
            });
            ConflictTransition::PickField { targets }
        }
    }

    pub(crate) fn submit_field(&mut self, values: Vec<String>) -> ConflictTransition {
        let Some(field) = values.first().filter(|value| !value.is_empty()).cloned() else {
            return ConflictTransition::Message("no conflict field selected".to_string());
        };
        match self.flow.take() {
            Some(ConflictFlow::PickVariant { choice, targets }) => {
                let Some(target) = targets.into_iter().find(|target| target.field == field) else {
                    return ConflictTransition::Message(format!("no conflict for field={field}"));
                };
                self.flow = Some(ConflictFlow::ConfirmVariant {
                    choice,
                    target: target.clone(),
                });
                ConflictTransition::Confirm { choice, target }
            }
            Some(ConflictFlow::PickManual { targets }) => {
                let Some(target) = targets.into_iter().find(|target| target.field == field) else {
                    return ConflictTransition::Message(format!("no conflict for field={field}"));
                };
                self.flow = Some(ConflictFlow::EditManual {
                    target: target.clone(),
                });
                ConflictTransition::EditManual { target }
            }
            _ => ConflictTransition::Message("conflict field picker is not active".to_string()),
        }
    }

    pub(crate) fn submit_confirmed_variant(&mut self) -> ConflictSubmit {
        let Some(ConflictFlow::ConfirmVariant { choice, target }) = self.flow.take() else {
            return ConflictSubmit::Inactive {
                message: "conflict confirmation is not active",
            };
        };
        let value = match choice {
            ConflictResolutionChoice::Local => target.local_value.clone(),
            ConflictResolutionChoice::Remote => target.remote_value.clone(),
        };
        ConflictSubmit::Resolve { target, value }
    }

    pub(crate) fn submit_manual_value(&mut self, value: String) -> ConflictSubmit {
        let Some(ConflictFlow::EditManual { target }) = self.flow.take() else {
            return ConflictSubmit::Inactive {
                message: "manual conflict edit is not active",
            };
        };
        ConflictSubmit::Resolve { target, value }
    }

    pub(crate) fn retry_manual_edit(&mut self, target: ConflictTarget) -> ConflictTransition {
        self.flow = Some(ConflictFlow::EditManual {
            target: target.clone(),
        });
        ConflictTransition::EditManual { target }
    }

    pub(crate) fn clear(&mut self) {
        self.flow = None;
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.flow.is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.flow.is_none()
    }
}

pub(crate) fn truncate_value_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conflict_target(field: &str) -> ConflictTarget {
        ConflictTarget {
            task_id: "task-1".to_string(),
            display_ref: "APP-1234".to_string(),
            field: field.to_string(),
            variant_a: "local".to_string(),
            local_value: "local value".to_string(),
            variant_b: "remote".to_string(),
            remote_value: "remote value".to_string(),
        }
    }

    #[test]
    fn inactive_confirm_reports_existing_message() {
        let mut state = ConflictFlowState::default();
        assert!(matches!(
            state.submit_confirmed_variant(),
            ConflictSubmit::Inactive {
                message: "conflict confirmation is not active"
            }
        ));
    }

    #[test]
    fn field_picker_unknown_field_reports_message() {
        let target = test_conflict_target("title");
        let other = test_conflict_target("description");
        let mut state = ConflictFlowState::default();
        state.begin_resolution(ConflictResolutionChoice::Local, vec![target, other]);
        assert!(matches!(
            state.submit_field(vec!["missing".to_string()]),
            ConflictTransition::Message(message) if message == "no conflict for field=missing"
        ));
    }

    #[test]
    fn begin_variant_single_target_skips_field_picker() {
        let target = test_conflict_target("title");
        let mut state = ConflictFlowState::default();
        assert!(matches!(
            state.begin_resolution(ConflictResolutionChoice::Local, vec![target.clone()]),
            ConflictTransition::Confirm {
                choice: ConflictResolutionChoice::Local,
                ..
            }
        ));
        assert!(state.is_active());
    }

    #[test]
    fn manual_submit_inactive_returns_manual_conflict_edit_is_not_active() {
        let mut state = ConflictFlowState::default();
        assert!(matches!(
            state.submit_manual_value("value".to_string()),
            ConflictSubmit::Inactive {
                message: "manual conflict edit is not active"
            }
        ));
    }

    #[test]
    fn truncate_value_preview_uses_character_count() {
        assert_eq!(truncate_value_preview("abc", 5), "abc");
        assert_eq!(truncate_value_preview("abcdef", 3), "abc…");
    }
}

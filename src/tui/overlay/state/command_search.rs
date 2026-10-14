use crate::query::SearchMatchedField;
use crate::tui::event::Action;
use crate::tui::overlay::text_input::LineEdit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchResultItem {
    pub(crate) task_id: crate::ids::TaskId,
    pub(crate) display_ref: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project_key: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) created_at: String,
    pub(crate) labels: Vec<String>,
    pub(crate) matched_field: SearchMatchedField,
    pub(crate) snippet: Option<String>,
    pub(crate) score: i64,
    pub(crate) deleted: bool,
    pub(crate) is_epic: bool,
    pub(crate) unavailable_reason: Option<String>,
    pub(crate) create_new: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchIntent {
    Navigate,
    AddDependency {
        selection: crate::tui::task_selection::TaskSelection,
        display_ref: String,
    },
    AddRelated {
        selection: crate::tui::task_selection::TaskSelection,
        display_ref: String,
    },
    AddEpicChild {
        epic_id: crate::ids::TaskId,
        display_ref: String,
        project_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchState {
    pub(crate) input: LineEdit,
    pub(crate) results: Vec<SearchResultItem>,
    pub(crate) selected: usize,
    pub(crate) total_matches: usize,
    pub(crate) results_query: Option<String>,
    pub(crate) intent: SearchIntent,
}

impl SearchState {
    pub(crate) fn blank() -> Self {
        Self::for_intent(SearchIntent::Navigate)
    }

    pub(crate) fn for_intent(intent: SearchIntent) -> Self {
        Self {
            input: LineEdit::blank(),
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            results_query: None,
            intent,
        }
    }

    pub(crate) fn current_query(&self) -> String {
        self.input.text.trim().to_string()
    }

    pub(crate) fn clear_results(&mut self) {
        self.results.clear();
        self.selected = 0;
        self.total_matches = 0;
        self.results_query = None;
    }

    pub(crate) fn selected_result(&self) -> Option<&SearchResultItem> {
        self.results.get(self.selected)
    }

    pub(crate) fn results_are_current(&self) -> bool {
        self.results_query.as_deref() == Some(self.input.text.trim())
    }

    pub(crate) fn selected_current_result(&self) -> Option<&SearchResultItem> {
        self.results_are_current()
            .then(|| self.selected_result())
            .flatten()
    }

    pub(crate) fn normalize_selection(&mut self) {
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.results.len() - 1);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandAvailabilityOverride {
    pub(crate) action: Action,
    pub(crate) reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandState {
    pub(crate) input: LineEdit,
    pub(crate) session: crate::tui::event::CommandSessionSnapshot,
    pub(crate) catalog: std::sync::Arc<crate::tui::event::CommandCatalog>,
    pub(crate) candidates: Vec<crate::tui::event::CommandCandidate>,
    pub(crate) cycle_input: Option<String>,
    pub(crate) cycle_candidates: Vec<crate::tui::event::CommandCandidate>,
    pub(crate) cycle_index: usize,
    pub(crate) highlighted: Option<usize>,
    pub(crate) unavailable: Vec<CommandAvailabilityOverride>,
}

impl CommandState {
    pub(crate) fn new(
        session: crate::tui::event::CommandSessionSnapshot,
        catalog: std::sync::Arc<crate::tui::event::CommandCatalog>,
        unavailable: Vec<CommandAvailabilityOverride>,
    ) -> Self {
        let mut state = Self {
            input: LineEdit::blank(),
            session,
            catalog,
            candidates: Vec::new(),
            cycle_input: None,
            cycle_candidates: Vec::new(),
            cycle_index: 0,
            highlighted: None,
            unavailable,
        };
        state.refresh_candidates();
        state
    }

    pub(crate) fn refresh_candidates(&mut self) {
        let unavailable = self
            .unavailable
            .iter()
            .map(|override_| (override_.action, override_.reason))
            .collect::<Vec<_>>();
        // Completion keeps the original matches visible until editing resets
        // the cycle, even as the input shows the highlighted candidate.
        let input = self.cycle_input.as_deref().unwrap_or(self.input.as_str());
        self.candidates = self.catalog.query(crate::tui::event::CommandQuery {
            input,
            snapshot: &self.session,
            unavailable: &unavailable,
        });
        self.highlighted = self
            .highlighted
            .filter(|index| *index < self.candidates.len());
    }

    pub(crate) fn begin_arguments(&mut self, name: &str) {
        self.input = LineEdit::new(format!("{name} "));
        self.reset_cycle();
        self.refresh_candidates();
    }

    pub(crate) fn reset_cycle(&mut self) {
        self.cycle_input = None;
        self.cycle_candidates.clear();
        self.cycle_index = 0;
        self.highlighted = None;
    }

    #[cfg(test)]
    pub(crate) fn test_with_input(input: &str) -> Self {
        let workspace = crate::tui::event::CommandWorkspaceSnapshot {
            id: crate::ids::WorkspaceId::new(),
            key: "test".to_string(),
            name: "Test".to_string(),
        };
        let session = crate::tui::event::CommandSessionSnapshot {
            workspace,
            surface: crate::tui::event::CommandSurfaceSnapshot::List {
                primary_task_id: None,
                marked_task_ids: Vec::new(),
                visible_task_ids: Vec::new(),
                focused_sidebar: None,
                is_empty: false,
                empty_preferred_action: None,
            },
            recurrence_series_id: None,
        };
        let mut state = Self::new(
            session,
            std::sync::Arc::new(crate::tui::event::CommandCatalog::default()),
            Vec::new(),
        );
        state.input = LineEdit::new(input.to_string());
        state.refresh_candidates();
        state
    }

    #[cfg(test)]
    pub(crate) fn marked_task_count(&self) -> usize {
        self.session.marked_task_ids().len()
    }

    #[cfg(test)]
    pub(crate) fn highlighted_name(&self) -> Option<&str> {
        self.highlighted
            .and_then(|row| self.candidates.get(row))
            .and_then(|candidate| self.catalog.command(candidate.index))
            .map(crate::tui::event::CatalogCommand::name)
    }
}

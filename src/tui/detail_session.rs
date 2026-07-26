use std::collections::BTreeSet;

use crate::tui::app::{DetailSection, DetailTargetId, RemovedEpicChild};
use crate::tui::bounded_history::BoundedHistory;
use crate::tui::detail_selection::{DetailTextSelection, TextCell};
use crate::tui::store::TaskViewState;

const DETAIL_HISTORY_LIMIT: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailSnapshot {
    pub(crate) task_id: crate::ids::TaskId,
    pub(crate) scroll: u16,
    pub(crate) focused_target: Option<DetailTargetId>,
    pub(crate) expanded_sections: BTreeSet<DetailSection>,
    pub(crate) view_state: TaskViewState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DetailTargetActivation {
    FollowTask(crate::ids::TaskId),
    OpenAttachment(String),
    ToggleSection(DetailSection),
}

pub(crate) enum DetailSession {
    Inactive,
    Active { state: Box<DetailState> },
}

impl DetailSession {
    pub(crate) fn inactive() -> Self {
        Self::Inactive
    }

    pub(crate) fn open(scroll: u16) -> Self {
        Self::Active {
            state: Box::new(DetailState::new(scroll)),
        }
    }

    pub(crate) fn close(&mut self) {
        *self = Self::inactive();
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    pub(crate) fn is_inactive(&self) -> bool {
        !self.is_active()
    }

    pub(crate) fn state(&self) -> Option<&DetailState> {
        match self {
            Self::Active { state } => Some(state),
            Self::Inactive => None,
        }
    }

    pub(crate) fn state_mut(&mut self) -> Option<&mut DetailState> {
        match self {
            Self::Active { state } => Some(state),
            Self::Inactive => None,
        }
    }
}

pub(crate) struct DetailState {
    pub(crate) scroll: u16,
    pub(crate) focused_target: Option<DetailTargetId>,
    pub(crate) hovered_target: Option<DetailTargetId>,
    pub(crate) expanded_sections: BTreeSet<DetailSection>,
    pub(crate) text_selection: Option<DetailTextSelection>,
    pub(crate) text_dragging: bool,
    pub(crate) history: BoundedHistory<DetailSnapshot>,
    pub(crate) removed_epic_child: Option<RemovedEpicChild>,
}

impl DetailState {
    fn new(scroll: u16) -> Self {
        Self {
            scroll,
            focused_target: None,
            hovered_target: None,
            expanded_sections: BTreeSet::new(),
            text_selection: None,
            text_dragging: false,
            history: BoundedHistory::new(DETAIL_HISTORY_LIMIT),
            removed_epic_child: None,
        }
    }

    pub(crate) fn scroll(&self) -> u16 {
        self.scroll
    }

    pub(crate) fn set_scroll(&mut self, scroll: u16) {
        self.scroll = scroll;
    }

    pub(crate) fn focused_target(&self) -> Option<&DetailTargetId> {
        self.focused_target.as_ref()
    }

    pub(crate) fn set_focused_target(&mut self, target: Option<DetailTargetId>) {
        self.focused_target = target;
    }

    pub(crate) fn hovered_target(&self) -> Option<&DetailTargetId> {
        self.hovered_target.as_ref()
    }

    pub(crate) fn set_hovered_target(&mut self, target: Option<DetailTargetId>) {
        self.hovered_target = target;
    }

    pub(crate) fn expanded_sections(&self) -> &BTreeSet<DetailSection> {
        &self.expanded_sections
    }

    pub(crate) fn text_selection(&self) -> Option<&DetailTextSelection> {
        self.text_selection.as_ref()
    }

    pub(crate) fn clear_text_selection(&mut self) -> bool {
        self.text_dragging = false;
        self.text_selection.take().is_some()
    }

    pub(crate) fn begin_text_selection(
        &mut self,
        task_id: crate::ids::TaskId,
        terminal_width: u16,
        cell: TextCell,
    ) {
        self.text_selection = Some(DetailTextSelection::new(task_id, terminal_width, cell));
        self.text_dragging = true;
    }

    pub(crate) fn update_text_selection(
        &mut self,
        task_id: &crate::ids::TaskId,
        terminal_width: u16,
        cell: TextCell,
    ) {
        if !self.text_dragging {
            return;
        }
        if let Some(selection) = self.text_selection.as_mut()
            && &selection.task_id == task_id
            && selection.terminal_width == terminal_width
        {
            selection.focus = cell;
        }
    }

    pub(crate) fn text_dragging(&self) -> bool {
        self.text_dragging
    }

    pub(crate) fn finish_text_drag(&mut self) {
        self.text_dragging = false;
    }

    pub(crate) fn snapshot(
        &self,
        task_id: crate::ids::TaskId,
        view_state: TaskViewState,
    ) -> DetailSnapshot {
        DetailSnapshot {
            task_id,
            scroll: self.scroll,
            focused_target: self.focused_target.clone(),
            expanded_sections: self.expanded_sections.clone(),
            view_state,
        }
    }

    pub(crate) fn has_parent(&self) -> bool {
        !self.history.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn push_history(&mut self, previous: DetailSnapshot) {
        self.history.push(previous);
    }

    pub(crate) fn follow_link(&mut self, previous: DetailSnapshot) {
        self.history.push(previous);
        self.scroll = 0;
        self.focused_target = None;
        self.hovered_target = None;
        self.expanded_sections.clear();
        self.text_dragging = false;
    }

    pub(crate) fn pop_history(&mut self) -> Option<DetailSnapshot> {
        self.history.pop()
    }

    pub(crate) fn restore_snapshot(&mut self, snapshot: &DetailSnapshot) {
        self.scroll = snapshot.scroll;
        self.focused_target = snapshot.focused_target.clone();
        self.hovered_target = None;
        self.expanded_sections = snapshot.expanded_sections.clone();
        self.text_dragging = false;
    }

    pub(crate) fn reset_task_state(&mut self, scroll: u16) {
        self.scroll = scroll;
        self.focused_target = None;
        self.hovered_target = None;
        self.expanded_sections.clear();
        self.text_selection = None;
        self.text_dragging = false;
    }

    pub(crate) fn selected_target(&self, targets: &[DetailTargetId]) -> Option<DetailTargetId> {
        let selected = self.focused_target.clone()?;
        targets.contains(&selected).then_some(selected)
    }

    pub(crate) fn focus_section(&mut self, targets: &[DetailTargetId], reverse: bool) -> bool {
        if targets.is_empty() {
            return false;
        }
        let sections = targets.iter().map(DetailTargetId::section).fold(
            Vec::new(),
            |mut sections, section| {
                if sections.last() != Some(&section) {
                    sections.push(section);
                }
                sections
            },
        );
        let next_section = self
            .focused_target
            .as_ref()
            .map(DetailTargetId::section)
            .and_then(|section| sections.iter().position(|candidate| *candidate == section))
            .map(|index| {
                let delta = if reverse { -1 } else { 1 };
                sections[(index as isize + delta).rem_euclid(sections.len() as isize) as usize]
            })
            .unwrap_or_else(|| {
                if reverse {
                    *sections.last().expect("non-empty detail sections")
                } else {
                    sections[0]
                }
            });
        self.clear_text_selection();
        self.focused_target = if reverse {
            targets
                .iter()
                .rev()
                .find(|target| target.section() == next_section)
                .cloned()
        } else {
            targets
                .iter()
                .find(|target| target.section() == next_section)
                .cloned()
        };
        true
    }

    pub(crate) fn move_focus(&mut self, targets: &[DetailTargetId], delta: isize) -> bool {
        let Some(selected) = self.focused_target.clone() else {
            return false;
        };
        let Some(index) = targets.iter().position(|target| target == &selected) else {
            self.focused_target = None;
            return false;
        };
        let next = (index as isize + delta).rem_euclid(targets.len() as isize) as usize;
        self.focused_target = Some(targets[next].clone());
        true
    }

    pub(crate) fn toggle_section(&mut self, section: DetailSection) -> bool {
        if self.expanded_sections.insert(section) {
            true
        } else {
            self.expanded_sections.remove(&section);
            self.focused_target = Some(DetailTargetId::Expand { section });
            false
        }
    }

    pub(crate) fn activate_target(&mut self, target: DetailTargetId) -> DetailTargetActivation {
        self.text_selection = None;
        self.text_dragging = false;
        self.focused_target = Some(target.clone());
        match target {
            DetailTargetId::Task { task_id, .. } => DetailTargetActivation::FollowTask(task_id),
            DetailTargetId::Attachment { attachment_id } => {
                DetailTargetActivation::OpenAttachment(attachment_id)
            }
            DetailTargetId::Expand { section } => DetailTargetActivation::ToggleSection(section),
        }
    }

    pub(crate) fn removed_epic_child(&self) -> Option<&RemovedEpicChild> {
        self.removed_epic_child.as_ref()
    }

    pub(crate) fn set_removed_epic_child(&mut self, removed: Option<RemovedEpicChild>) {
        self.removed_epic_child = removed;
    }

    pub(crate) fn take_removed_epic_child(&mut self) -> Option<RemovedEpicChild> {
        self.removed_epic_child.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::detail_selection::TextCell;

    fn parsed_task_id(value: &str) -> crate::ids::TaskId {
        value.parse().unwrap()
    }

    #[test]
    fn open_and_reset_establish_clean_task_state() {
        let mut session = DetailState::new(7);
        session.set_focused_target(Some(DetailTargetId::Expand {
            section: DetailSection::Blocks,
        }));
        session.expanded_sections.insert(DetailSection::Blocks);
        session.begin_text_selection(
            parsed_task_id("ABCD000000000000"),
            80,
            TextCell { start: 0, end: 1 },
        );

        session.reset_task_state(0);

        assert_eq!(session.scroll(), 0);
        assert!(session.focused_target().is_none());
        assert!(session.expanded_sections().is_empty());
        assert!(session.text_selection().is_none());
        assert!(!session.text_dragging());
    }

    #[test]
    fn text_selection_updates_only_for_matching_frame() {
        let task_id = parsed_task_id("ABCD000000000000");
        let other_id = parsed_task_id("ABCD000000000001");
        let mut session = DetailState::new(0);
        session.begin_text_selection(task_id.clone(), 80, TextCell { start: 1, end: 2 });

        session.update_text_selection(&other_id, 80, TextCell { start: 3, end: 4 });
        session.update_text_selection(&task_id, 79, TextCell { start: 5, end: 6 });
        assert_eq!(session.text_selection().unwrap().focus.start, 1);

        session.update_text_selection(&task_id, 80, TextCell { start: 7, end: 8 });
        assert_eq!(session.text_selection().unwrap().focus.start, 7);
    }

    #[test]
    fn activity_lifecycle_owns_active_state() {
        let mut detail = DetailSession::open(4);
        assert!(detail.is_active());
        assert_eq!(detail.state().unwrap().scroll(), 4);

        detail.close();
        assert!(detail.is_inactive());
    }

    #[test]
    fn linked_history_restores_state_and_exhausts_cleanly() {
        let task_id = parsed_task_id("ABCD000000000000");
        let focused = DetailTargetId::Expand {
            section: DetailSection::DependsOn,
        };
        let entry = DetailSnapshot {
            task_id,
            scroll: 6,
            focused_target: Some(focused.clone()),
            expanded_sections: [DetailSection::DependsOn].into_iter().collect(),
            view_state: TaskViewState::default(),
        };
        let mut session = DetailState::new(2);

        session.follow_link(entry.clone());
        assert!(session.has_parent());
        assert_eq!(session.scroll(), 0);

        let restored = session.pop_history().unwrap();
        session.restore_snapshot(&restored);
        assert_eq!(session.scroll(), 6);
        assert_eq!(session.focused_target(), Some(&focused));
        assert!(
            session
                .expanded_sections()
                .contains(&DetailSection::DependsOn)
        );
        assert!(session.pop_history().is_none());
    }

    #[test]
    fn target_activation_clears_selection_and_tracks_focus() {
        let task_id = parsed_task_id("ABCD000000000000");
        let target = DetailTargetId::Task {
            section: DetailSection::Blocks,
            task_id: task_id.clone(),
        };
        let mut session = DetailState::new(0);
        session.begin_text_selection(task_id.clone(), 80, TextCell { start: 0, end: 1 });

        assert_eq!(
            session.activate_target(target.clone()),
            DetailTargetActivation::FollowTask(task_id)
        );
        assert_eq!(session.focused_target(), Some(&target));
        assert!(session.text_selection().is_none());
        assert!(!session.text_dragging());
    }
}

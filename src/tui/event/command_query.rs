use super::command_catalog::{CatalogCommand, CommandCatalog, command_match_rank};
use super::{
    Action, BulkSupport, COMMANDS, CommandScopePolicy, CommandSessionSnapshot, CommandSituation,
    DetailCommandFocus, DetailFocusPolicy, RelationshipTargetPolicy,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandDisabled {
    AvailableOnlyInTaskList,
    RelationshipMismatch,
    FocusedAttachment,
    AttachmentBytesUnavailable,
    Other(String),
}

impl CommandDisabled {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::AvailableOnlyInTaskList => "available only in the task list",
            Self::RelationshipMismatch => "command does not apply to the captured relationship",
            Self::FocusedAttachment => "requires a focused attachment",
            Self::AttachmentBytesUnavailable => "attachment bytes are unavailable",
            Self::Other(reason) => reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandAvailability {
    Ready,
    Disabled(CommandDisabled),
}

impl CommandAvailability {
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Ready => None,
            Self::Disabled(reason) => Some(reason.message()),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::Ready => 0,
            Self::Disabled(_) => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandCandidate {
    pub(crate) index: usize,
    pub(crate) availability: CommandAvailability,
}

pub(crate) struct CommandQuery<'a> {
    pub(crate) input: &'a str,
    pub(crate) snapshot: &'a CommandSessionSnapshot,
    pub(crate) unavailable: &'a [(Action, &'static str)],
}

impl CommandCatalog {
    pub(crate) fn query(&self, query: CommandQuery<'_>) -> Vec<CommandCandidate> {
        let input = normalize_query(query.input);
        let situation = query.snapshot.situation();
        let mut candidates = self
            .all_commands()
            .enumerate()
            .filter(|(_, command)| panel_applicable(*command, &situation))
            .filter_map(|(index, command)| {
                let text_rank = command_match_rank(command, input)?;
                let availability = command_availability(command, query.snapshot, query.unavailable);
                if input.is_empty() && availability != CommandAvailability::Ready {
                    return None;
                }
                Some((
                    text_rank,
                    availability.rank(),
                    contextual_rank(command, &situation),
                    catalog_order(command),
                    index,
                    availability,
                ))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.cmp(&right.1))
                .then(left.2.cmp(&right.2))
                .then(left.3.cmp(&right.3))
                .then(left.4.cmp(&right.4))
        });
        candidates
            .into_iter()
            .map(|(_, _, _, _, index, availability)| CommandCandidate {
                index,
                availability,
            })
            .collect()
    }
}

fn disabled(reason: impl Into<String>) -> CommandAvailability {
    let reason = reason.into();
    let kind = match reason.as_str() {
        "available only in the task list" => CommandDisabled::AvailableOnlyInTaskList,
        "command does not apply to the captured relationship" => {
            CommandDisabled::RelationshipMismatch
        }
        "requires a focused attachment" => CommandDisabled::FocusedAttachment,
        "attachment bytes are unavailable" => CommandDisabled::AttachmentBytesUnavailable,
        _ => CommandDisabled::Other(reason),
    };
    CommandAvailability::Disabled(kind)
}

fn panel_applicable(command: CatalogCommand<'_>, situation: &CommandSituation) -> bool {
    if command.is_custom() {
        return true;
    }
    let action = command.built_in().expect("built-in command").action;
    match situation {
        CommandSituation::AddTaskOnly => matches!(
            action,
            Action::BeginAddTask | Action::Quit | Action::ToggleHelp | Action::ShowConfigInfo
        ),
        _ => true,
    }
}

pub(crate) fn command_availability(
    command: CatalogCommand<'_>,
    snapshot: &CommandSessionSnapshot,
    overrides: &[(Action, &'static str)],
) -> CommandAvailability {
    let captured_marked = snapshot.marked_task_ids().len();
    let has_primary = snapshot.primary_task_id().is_some();
    let Some(spec) = command.built_in() else {
        return command
            .unavailable_reason(has_primary, captured_marked)
            .map_or(CommandAvailability::Ready, |reason| {
                disabled(reason.to_string())
            });
    };
    let domain = snapshot.routing_domain();
    let activates_sidebar =
        spec.action == Action::ToggleDetail && snapshot.sidebar_target().is_some();
    if spec.scope_policy() == CommandScopePolicy::ListOnly && domain != super::RoutingDomain::Normal
    {
        return disabled("available only in the task list".to_string());
    }
    let marked = if matches!(
        snapshot.surface,
        super::CommandSurfaceSnapshot::Detail { .. }
    ) {
        0
    } else {
        captured_marked
    };
    if let Some(reason) = command.unavailable_reason(has_primary, marked) {
        return disabled(reason.to_string());
    }
    if let Some((_, reason)) = overrides.iter().find(|(action, _)| *action == spec.action) {
        return disabled((*reason).to_string());
    }
    let relationship_matches = match (spec.target_policy(), snapshot.detail_focus()) {
        (
            super::CommandTargetPolicy::Relationship(RelationshipTargetPolicy::Dependency),
            Some(DetailCommandFocus::Relationship {
                section:
                    crate::tui::app::DetailSection::DependsOn | crate::tui::app::DetailSection::Blocks,
                ..
            }),
        )
        | (
            super::CommandTargetPolicy::Relationship(RelationshipTargetPolicy::Related),
            Some(DetailCommandFocus::Relationship {
                section: crate::tui::app::DetailSection::Related,
                ..
            }),
        )
        | (
            super::CommandTargetPolicy::Relationship(RelationshipTargetPolicy::EpicChild),
            Some(
                DetailCommandFocus::Relationship {
                    section: crate::tui::app::DetailSection::EpicParent,
                    ..
                }
                | DetailCommandFocus::Relationship {
                    section: crate::tui::app::DetailSection::EpicChildren,
                    ..
                },
            ),
        ) => Some(true),
        (super::CommandTargetPolicy::Relationship(_), Some(_)) => Some(false),
        _ => None,
    };
    if relationship_matches == Some(false) {
        return disabled("command does not apply to the captured relationship".to_string());
    }
    let recurrence_action = spec.action.recurrence_kind().is_some();
    if recurrence_action && snapshot.recurrence_series_id.is_none() {
        return disabled("requires a recurring task or series".to_string());
    }
    if recurrence_action {
        return CommandAvailability::Ready;
    }
    match spec.action.bulk_support() {
        BulkSupport::Batch if marked == 0 && !has_primary => {
            return disabled("requires a selected task".to_string());
        }
        BulkSupport::SingleOnly(_) if marked > 1 => {
            return disabled("requires one task".to_string());
        }
        BulkSupport::SingleOnly(_) | BulkSupport::Focused if !has_primary && !activates_sidebar => {
            return disabled("requires a selected task".to_string());
        }
        BulkSupport::BulkControl if spec.action == Action::ClearMarks && marked == 0 => {
            return disabled("requires one or more marked tasks".to_string());
        }
        BulkSupport::BulkControl if spec.action == Action::ToggleMarkSelected && !has_primary => {
            return disabled("requires a selected task".to_string());
        }
        BulkSupport::BulkControl
            if spec.action == Action::ToggleMarkAllInView && snapshot.is_empty() =>
        {
            return disabled("requires tasks in the current view".to_string());
        }
        _ => {}
    }
    let focused_section = match snapshot.detail_focus() {
        Some(DetailCommandFocus::Relationship { section, .. }) => Some(*section),
        _ => None,
    };
    if !super::command_catalog::focus_policy_compatible(
        spec.detail_focus(),
        domain,
        focused_section,
    ) {
        let reason = if spec.detail_focus() == DetailFocusPolicy::Attachment {
            "requires a focused attachment"
        } else if domain == super::RoutingDomain::DetailRelated {
            "open the related task before using this command"
        } else {
            "requires parent task or relationship focus"
        };
        return disabled(reason.to_string());
    }
    if spec.action == Action::SaveAttachment
        && matches!(
            snapshot.detail_focus(),
            Some(DetailCommandFocus::Attachment {
                bytes_present: false,
                ..
            })
        )
    {
        return disabled("attachment bytes are unavailable".to_string());
    }
    CommandAvailability::Ready
}

const EPIC_RELATIONSHIP_LEADERS: &[&str] = &[
    "status-picker",
    "status-done",
    "edit-title",
    "copy-ref",
    "copy-title",
    "task-child-remove",
    "delete",
    "add-note",
];
const DEPENDENCY_RELATIONSHIP_LEADERS: &[&str] = &[
    "status-picker",
    "status-done",
    "edit-title",
    "copy-ref",
    "copy-title",
    "remove-dependency",
    "delete",
    "add-note",
];
const RELATED_TASK_RELATIONSHIP_LEADERS: &[&str] = &[
    "status-picker",
    "status-done",
    "edit-title",
    "copy-ref",
    "copy-title",
    "remove-related",
    "delete",
    "add-note",
];
const NOTE_DETAIL_LEADERS: &[&str] = &["back", "search", "refresh", "command", "undo", "quit"];
const DISCLOSURE_DETAIL_LEADERS: &[&str] = &["back", "search", "refresh", "command", "quit"];

fn contextual_rank(command: CatalogCommand<'_>, situation: &CommandSituation) -> (u8, usize) {
    let leaders: &[&str] = match situation {
        CommandSituation::ParentDetail => &[
            "edit-title",
            "edit-description",
            "status-picker",
            "edit-priority",
            "edit-project",
            "edit-labels",
            "add-note",
            "add-dependency",
        ],
        CommandSituation::Relationship { section } => match section {
            crate::tui::app::DetailSection::EpicParent
            | crate::tui::app::DetailSection::EpicChildren => EPIC_RELATIONSHIP_LEADERS,
            crate::tui::app::DetailSection::Related => RELATED_TASK_RELATIONSHIP_LEADERS,
            _ => DEPENDENCY_RELATIONSHIP_LEADERS,
        },
        CommandSituation::Attachment => &[
            "attachment-open",
            "attachment-save",
            "attachment-delete",
            "back",
            "search",
            "refresh",
            "command",
            "quit",
        ],
        CommandSituation::Note => NOTE_DETAIL_LEADERS,
        CommandSituation::Disclosure => DISCLOSURE_DETAIL_LEADERS,
        CommandSituation::MarkedSelection { .. } => &[
            "status-picker",
            "status-done",
            "edit-project",
            "edit-priority",
            "edit-labels",
            "delete",
            "copy-ref",
            "clear-marks",
        ],
        CommandSituation::SidebarProject { .. } => &[
            "scope-project",
            "rename-project",
            "add-task",
            "add-project-path",
            "remove-project-path",
            "delete-project",
            "scope-all",
            "workspace-switch",
        ],
        CommandSituation::SidebarWorkspace => &[
            "scope-all",
            "workspace-switch",
            "workspace-rename",
            "workspace-create",
            "add-task",
            "scope-project",
            "view-queue",
            "search",
        ],
        CommandSituation::EmptyView { preferred_action } => match preferred_action {
            Some(Action::ClearFilters) => &[
                "filter-clear",
                "add-task",
                "search",
                "view-queue",
                "filter-label",
                "filter-priority",
                "refresh",
                "scope-all",
            ],
            Some(Action::BeginSearch) => &[
                "search",
                "add-task",
                "view-queue",
                "filter-clear",
                "scope-all",
                "refresh",
                "view-open",
                "view-inbox",
            ],
            Some(Action::ToggleDeletedFilter) => &[
                "filter-deleted",
                "add-task",
                "search",
                "view-queue",
                "filter-clear",
                "refresh",
                "scope-all",
                "view-open",
            ],
            _ => &[
                "add-task",
                "search",
                "view-queue",
                "filter-clear",
                "scope-project",
                "refresh",
                "view-open",
                "view-inbox",
            ],
        },
        CommandSituation::SelectedTask => &[
            "status-picker",
            "status-done",
            "edit-title",
            "edit-priority",
            "edit-project",
            "edit-labels",
            "add-note",
            "delete",
        ],
        CommandSituation::RecurringRow => &[
            "recurrence-edit-template",
            "recurrence-pause",
            "recurrence-resume",
            "recurrence-stop",
            "recurrence-history",
            "recurrence-skip",
            "add-task",
            "search",
        ],
        CommandSituation::SidebarView { view } => return sidebar_view_rank(command, *view),
        CommandSituation::AddTaskOnly => &["add-task", "quit", "help", "config-show"],
        CommandSituation::Neutral => &[
            "add-task",
            "search",
            "view-queue",
            "scope-project",
            "view-open",
            "refresh",
            "help",
            "quit",
        ],
    };
    leaders
        .iter()
        .position(|name| *name == command.name())
        .map_or((1, semantic_band(command)), |index| (0, index))
}

fn sidebar_view_rank(
    command: CatalogCommand<'_>,
    view: crate::tui::store::TaskQuery,
) -> (u8, usize) {
    let action = command.built_in().map(|command| command.action);
    if action == Some(Action::ShowView(view)) {
        return (0, 0);
    }
    let leaders = [
        "detail",
        "search",
        "scope-project",
        "scope-all",
        "filter-clear",
        "toggle-sidebar",
        "add-task",
    ];
    leaders
        .iter()
        .position(|name| *name == command.name())
        .map_or((1, semantic_band(command)), |index| (0, index + 1))
}

fn semantic_band(command: CatalogCommand<'_>) -> usize {
    match command.bulk_support() {
        BulkSupport::Batch => 10,
        BulkSupport::Focused => 20,
        BulkSupport::BulkControl => 30,
        BulkSupport::NotTaskScoped => 40,
        BulkSupport::SingleOnly(_) => 50,
    }
}

fn catalog_order(command: CatalogCommand<'_>) -> usize {
    match command {
        CatalogCommand::BuiltIn(spec) => COMMANDS
            .iter()
            .position(|candidate| candidate.name == spec.name)
            .unwrap_or(usize::MAX),
        CatalogCommand::Custom { id, .. } => COMMANDS.len().saturating_add(id),
    }
}

fn normalize_query(input: &str) -> &str {
    input.trim().strip_prefix(':').unwrap_or(input.trim())
}

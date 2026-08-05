use crate::config::{CustomTuiCommandConfig, CustomTuiCommandRequirement};

use super::{
    Action, BulkSupport, COMMANDS, CommandContext, CommandLifecycle, CommandSpec, KeySequence,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandHandler {
    BuiltIn(Action),
    Custom(usize),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CatalogCommand<'a> {
    BuiltIn(&'static CommandSpec),
    Custom {
        id: usize,
        command: &'a CustomTuiCommandConfig,
    },
}

impl<'a> CatalogCommand<'a> {
    pub(crate) fn name(self) -> &'a str {
        match self {
            Self::BuiltIn(command) => command.name,
            Self::Custom { command, .. } => &command.name,
        }
    }

    fn matches_alias(self, input: &str) -> bool {
        match self {
            Self::BuiltIn(command) => command.aliases.contains(&input),
            Self::Custom { command, .. } => command.aliases.iter().any(|alias| alias == input),
        }
    }

    fn alias_starts_with(self, input: &str) -> bool {
        match self {
            Self::BuiltIn(command) => command.aliases.iter().any(|alias| alias.starts_with(input)),
            Self::Custom { command, .. } => {
                command.aliases.iter().any(|alias| alias.starts_with(input))
            }
        }
    }

    fn alias_dashless_eq(self, input: &str) -> bool {
        match self {
            Self::BuiltIn(command) => command
                .aliases
                .iter()
                .any(|alias| dashless_eq(alias, input)),
            Self::Custom { command, .. } => command
                .aliases
                .iter()
                .any(|alias| dashless_eq(alias, input)),
        }
    }

    fn alias_dashless_starts_with(self, input: &str) -> bool {
        match self {
            Self::BuiltIn(command) => command
                .aliases
                .iter()
                .any(|alias| dashless_starts_with(alias, input)),
            Self::Custom { command, .. } => command
                .aliases
                .iter()
                .any(|alias| dashless_starts_with(alias, input)),
        }
    }

    pub(crate) fn description(self) -> &'a str {
        match self {
            Self::BuiltIn(command) => command.description,
            Self::Custom { command, .. } => &command.description,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn section(self) -> &'static str {
        match self {
            Self::BuiltIn(command) => command.section,
            Self::Custom { .. } => "Custom",
        }
    }

    pub(crate) fn keys(self, context: CommandContext) -> &'static [KeySequence] {
        match self {
            Self::BuiltIn(command) => command.keys(context),
            Self::Custom { .. } => &[],
        }
    }

    pub(crate) fn lifecycle(self) -> CommandLifecycle {
        match self {
            Self::BuiltIn(command) => command.lifecycle,
            Self::Custom { .. } => CommandLifecycle::Implemented,
        }
    }

    pub(crate) fn bulk_support(self) -> BulkSupport {
        match self {
            Self::BuiltIn(command) => command.bulk_support(),
            Self::Custom { command, .. } => match command.requires {
                CustomTuiCommandRequirement::None => BulkSupport::NotTaskScoped,
                CustomTuiCommandRequirement::SelectedTask => BulkSupport::Focused,
            },
        }
    }

    pub(crate) fn requires_selected_task(self) -> bool {
        matches!(
            self,
            Self::Custom { command, .. }
                if command.requires == CustomTuiCommandRequirement::SelectedTask
        )
    }

    pub(crate) fn handler(self) -> CommandHandler {
        match self {
            Self::BuiltIn(command) => CommandHandler::BuiltIn(command.action),
            Self::Custom { id, .. } => CommandHandler::Custom(id),
        }
    }

    pub(crate) fn built_in(self) -> Option<&'static CommandSpec> {
        match self {
            Self::BuiltIn(command) => Some(command),
            Self::Custom { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CommandCatalog {
    custom: Vec<CustomTuiCommandConfig>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CatalogLookup<'a> {
    Empty,
    Found(CatalogCommand<'a>),
    Ambiguous,
    Missing,
}

impl CommandCatalog {
    pub(crate) fn new(custom: Vec<CustomTuiCommandConfig>) -> Self {
        Self { custom }
    }

    pub(crate) fn custom(&self, id: usize) -> Option<&CustomTuiCommandConfig> {
        self.custom.get(id)
    }

    pub(crate) fn commands(&self, context: CommandContext) -> Vec<CatalogCommand<'_>> {
        let mut commands = COMMANDS
            .iter()
            .filter(|command| command.is_available(context))
            .map(CatalogCommand::BuiltIn)
            .collect::<Vec<_>>();
        commands.extend(
            self.custom
                .iter()
                .enumerate()
                .map(|(id, command)| CatalogCommand::Custom { id, command }),
        );
        commands
    }

    pub(crate) fn matching(
        &self,
        context: CommandContext,
        input: &str,
        marked_task_count: usize,
    ) -> Vec<CatalogCommand<'_>> {
        let input = normalize(input);
        let mut matches = self
            .commands(context)
            .into_iter()
            .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
            .collect::<Vec<_>>();
        matches.sort_by_key(|(rank, _)| *rank);
        let mut matches = matches
            .into_iter()
            .map(|(_, command)| command)
            .collect::<Vec<_>>();
        if marked_task_count > 0 && input.is_empty() {
            matches.sort_by_key(|command| match command.bulk_support() {
                BulkSupport::Batch => 0,
                BulkSupport::BulkControl => 1,
                BulkSupport::Focused => 2,
                BulkSupport::NotTaskScoped => 3,
                BulkSupport::SingleOnly(_) => 4,
            });
        }
        matches
    }

    pub(crate) fn lookup(&self, context: CommandContext, input: &str) -> CatalogLookup<'_> {
        let input = normalize(input);
        if input.is_empty() {
            return CatalogLookup::Empty;
        }
        let matches = self
            .commands(context)
            .into_iter()
            .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
            .collect::<Vec<_>>();
        let Some(best_rank) = matches.iter().map(|(rank, _)| *rank).min() else {
            return CatalogLookup::Missing;
        };
        let mut best = matches
            .into_iter()
            .filter(|(rank, _)| *rank == best_rank)
            .map(|(_, command)| command);
        let Some(command) = best.next() else {
            return CatalogLookup::Missing;
        };
        if best.next().is_some() {
            CatalogLookup::Ambiguous
        } else {
            CatalogLookup::Found(command)
        }
    }

    pub(crate) fn cycle_options(&self, context: CommandContext, input: &str) -> Vec<String> {
        self.matching(context, input, 0)
            .into_iter()
            .map(|command| command.name().to_string())
            .collect()
    }

    pub(crate) fn complete(
        &self,
        context: CommandContext,
        input: &str,
    ) -> super::CommandCompletion {
        let input = normalize(input);
        if input.is_empty() {
            return super::CommandCompletion::Empty;
        }
        let matches = self
            .commands(context)
            .into_iter()
            .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
            .collect::<Vec<_>>();
        let Some(best_rank) = matches.iter().map(|(rank, _)| *rank).min() else {
            return super::CommandCompletion::Missing;
        };
        let names = matches
            .iter()
            .filter(|(rank, _)| *rank == best_rank)
            .map(|(_, command)| command.name())
            .collect::<Vec<_>>();
        if names.len() != 1 {
            return super::CommandCompletion::Unchanged;
        }
        if names[0].len() > input.len() {
            super::CommandCompletion::Completed(names[0].to_string())
        } else {
            super::CommandCompletion::Unchanged
        }
    }
}

fn normalize(input: &str) -> &str {
    input.trim().strip_prefix(':').unwrap_or(input.trim())
}

fn command_match_rank(command: CatalogCommand<'_>, input: &str) -> Option<u8> {
    if input.is_empty() {
        return Some(0);
    }
    if command.name() == input || command.matches_alias(input) {
        Some(0)
    } else if command.name().starts_with(input)
        || command.alias_starts_with(input)
        || dashless_eq(command.name(), input)
        || command.alias_dashless_eq(input)
    {
        Some(1)
    } else if dashless_starts_with(command.name(), input)
        || command.alias_dashless_starts_with(input)
    {
        Some(2)
    } else if command
        .name()
        .split('-')
        .skip(1)
        .any(|segment| segment.starts_with(input))
    {
        Some(3)
    } else {
        None
    }
}

fn dashless_eq(value: &str, input: &str) -> bool {
    value.contains('-') && value.chars().filter(|ch| *ch != '-').eq(input.chars())
}

fn dashless_starts_with(value: &str, input: &str) -> bool {
    if !value.contains('-') {
        return false;
    }
    value
        .chars()
        .filter(|ch| *ch != '-')
        .collect::<String>()
        .starts_with(input)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};

    fn custom() -> CustomTuiCommandConfig {
        CustomTuiCommandConfig {
            name: "dispatch".to_string(),
            aliases: vec!["custom-dispatch".to_string()],
            description: "dispatch selected task".to_string(),
            program: PathBuf::from("dispatch-task"),
            args: vec![],
            requires: CustomTuiCommandRequirement::SelectedTask,
            execution: CustomTuiCommandExecution::Wait,
            on_success: CustomTuiCommandSuccess::Quit,
        }
    }

    #[test]
    fn custom_names_and_aliases_share_one_catalog_entry() {
        let catalog = CommandCatalog::new(vec![custom()]);
        let CatalogLookup::Found(canonical) = catalog.lookup(CommandContext::Normal, "dispatch")
        else {
            panic!("canonical command missing");
        };
        let CatalogLookup::Found(alias) = catalog.lookup(CommandContext::Detail, "custom-dispatch")
        else {
            panic!("alias missing");
        };

        assert_eq!(canonical.handler(), alias.handler());
        assert_eq!(canonical.name(), "dispatch");
    }

    #[test]
    fn matching_and_completion_include_custom_and_built_in_commands() {
        let catalog = CommandCatalog::new(vec![custom()]);
        let names = catalog
            .matching(CommandContext::Normal, "d", 0)
            .into_iter()
            .map(CatalogCommand::name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"dispatch"));
        assert!(names.contains(&"delete"));
        assert!(
            catalog
                .cycle_options(CommandContext::Normal, "custom-d")
                .contains(&"dispatch".to_string())
        );
    }
}

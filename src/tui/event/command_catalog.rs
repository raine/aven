use anyhow::{Result, bail};
use crossterm::event::KeyCode;

use crate::config::{CustomTuiCommandConfig, CustomTuiCommandTarget};

use super::{
    Action, BulkSupport, COMMANDS, CommandContext, CommandSpec, KeySequence, shortcut_label,
};

const MAX_CUSTOM_KEY_SEQUENCE_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandHandler {
    BuiltIn(Action),
    Custom(usize),
}

#[derive(Debug, Clone)]
struct RuntimeKeySequence {
    codes: Vec<KeyCode>,
    label: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeCustomCommand {
    config: CustomTuiCommandConfig,
    list_keys: Vec<RuntimeKeySequence>,
    detail_keys: Vec<RuntimeKeySequence>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CatalogKeySequence<'a> {
    pub(crate) codes: &'a [KeyCode],
    pub(crate) label: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CatalogCommand<'a> {
    BuiltIn(&'static CommandSpec),
    Custom {
        id: usize,
        command: &'a RuntimeCustomCommand,
    },
}

impl<'a> CatalogCommand<'a> {
    pub(crate) fn name(self) -> &'a str {
        match self {
            Self::BuiltIn(command) => command.name,
            Self::Custom { command, .. } => &command.config.name,
        }
    }

    fn matches_alias(self, input: &str) -> bool {
        match self {
            Self::BuiltIn(command) => command.aliases.contains(&input),
            Self::Custom { command, .. } => {
                command.config.aliases.iter().any(|alias| alias == input)
            }
        }
    }

    fn alias_starts_with(self, input: &str) -> bool {
        match self {
            Self::BuiltIn(command) => command.aliases.iter().any(|alias| alias.starts_with(input)),
            Self::Custom { command, .. } => command
                .config
                .aliases
                .iter()
                .any(|alias| alias.starts_with(input)),
        }
    }

    fn alias_dashless_eq(self, input: &str) -> bool {
        match self {
            Self::BuiltIn(command) => command
                .aliases
                .iter()
                .any(|alias| dashless_eq(alias, input)),
            Self::Custom { command, .. } => command
                .config
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
                .config
                .aliases
                .iter()
                .any(|alias| dashless_starts_with(alias, input)),
        }
    }

    pub(crate) fn description(self) -> &'a str {
        match self {
            Self::BuiltIn(command) => command.description,
            Self::Custom { command, .. } => &command.config.description,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn section(self) -> &'static str {
        match self {
            Self::BuiltIn(command) => command.section,
            Self::Custom { .. } => "Custom",
        }
    }

    pub(crate) fn keys(self, context: CommandContext) -> Vec<CatalogKeySequence<'a>> {
        match self {
            Self::BuiltIn(command) => command
                .keys(context)
                .iter()
                .map(catalog_static_key)
                .collect(),
            Self::Custom { command, .. } => command
                .keys(context)
                .iter()
                .map(|key| CatalogKeySequence {
                    codes: &key.codes,
                    label: &key.label,
                })
                .collect(),
        }
    }

    pub(crate) fn bulk_support(self) -> BulkSupport {
        match self {
            Self::BuiltIn(command) => command.bulk_support(),
            Self::Custom { command, .. } => match command.config.target {
                CustomTuiCommandTarget::None => BulkSupport::NotTaskScoped,
                CustomTuiCommandTarget::Focused => BulkSupport::Focused,
                CustomTuiCommandTarget::Marked | CustomTuiCommandTarget::MarkedOrFocused => {
                    BulkSupport::Batch
                }
            },
        }
    }

    pub(crate) fn custom_target(self) -> Option<CustomTuiCommandTarget> {
        match self {
            Self::BuiltIn(_) => None,
            Self::Custom { command, .. } => Some(command.config.target),
        }
    }

    pub(crate) fn unavailable_reason(
        self,
        has_primary_task: bool,
        marked_task_count: usize,
    ) -> Option<&'static str> {
        match self.custom_target()? {
            CustomTuiCommandTarget::None => None,
            CustomTuiCommandTarget::Focused if !has_primary_task => Some("requires a focused task"),
            CustomTuiCommandTarget::Marked if marked_task_count == 0 => {
                Some("requires one or more marked tasks")
            }
            CustomTuiCommandTarget::MarkedOrFocused
                if marked_task_count == 0 && !has_primary_task =>
            {
                Some("requires a marked or focused task")
            }
            CustomTuiCommandTarget::Focused
            | CustomTuiCommandTarget::Marked
            | CustomTuiCommandTarget::MarkedOrFocused => None,
        }
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

    pub(crate) fn is_custom(self) -> bool {
        matches!(self, Self::Custom { .. })
    }
}

impl RuntimeCustomCommand {
    fn new(config: CustomTuiCommandConfig) -> Self {
        let list_keys = parse_keys(&config.keys);
        let detail_keys = parse_keys(config.detail_keys.as_ref().unwrap_or(&config.keys));
        Self {
            config,
            list_keys,
            detail_keys,
        }
    }

    fn keys(&self, context: CommandContext) -> &[RuntimeKeySequence] {
        match context {
            CommandContext::Normal => &self.list_keys,
            CommandContext::Detail => &self.detail_keys,
        }
    }
}

fn catalog_static_key(key: &'static KeySequence) -> CatalogKeySequence<'static> {
    CatalogKeySequence {
        codes: key.codes,
        label: key.label,
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CommandCatalog {
    custom: Vec<RuntimeCustomCommand>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CatalogLookup<'a> {
    Empty,
    Found(CatalogCommand<'a>),
    Ambiguous,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogShortcutLookup {
    Found(CommandHandler),
    Prefix,
    Ambiguous(CommandHandler),
    Missing,
}

impl CommandCatalog {
    pub(crate) fn new(custom: Vec<CustomTuiCommandConfig>) -> Self {
        Self {
            custom: custom.into_iter().map(RuntimeCustomCommand::new).collect(),
        }
    }

    pub(crate) fn custom(&self, id: usize) -> Option<&CustomTuiCommandConfig> {
        self.custom.get(id).map(|command| &command.config)
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

    pub(crate) fn resolve_shortcut(
        &self,
        context: CommandContext,
        input: &[KeyCode],
    ) -> CatalogShortcutLookup {
        if input.is_empty() {
            return CatalogShortcutLookup::Missing;
        }
        let mut exact = Vec::new();
        let mut prefix = false;
        for command in self.commands(context) {
            for key in command.keys(context) {
                if key.codes == input {
                    exact.push(command.handler());
                } else if key.codes.starts_with(input) {
                    prefix = true;
                }
            }
        }
        match (exact.as_slice(), prefix) {
            ([handler], false) => CatalogShortcutLookup::Found(*handler),
            ([handler], true) => CatalogShortcutLookup::Ambiguous(*handler),
            ([handler, ..], _) => CatalogShortcutLookup::Ambiguous(*handler),
            ([], true) => CatalogShortcutLookup::Prefix,
            ([], false) => CatalogShortcutLookup::Missing,
        }
    }

    pub(crate) fn custom_shortcut_starts_with(
        &self,
        context: CommandContext,
        input: &[KeyCode],
    ) -> bool {
        self.custom.iter().any(|command| {
            command
                .keys(context)
                .iter()
                .any(|key| key.codes.starts_with(input))
        })
    }

    pub(crate) fn prefix_hints(
        &self,
        context: CommandContext,
        pending: &[String],
    ) -> Vec<(CatalogCommand<'_>, String)> {
        self.commands(context)
            .into_iter()
            .flat_map(|command| {
                command.keys(context).into_iter().filter_map(move |key| {
                    let labels = key
                        .codes
                        .iter()
                        .map(|code| super::key_label(*code))
                        .collect::<Vec<_>>();
                    if labels.len() <= pending.len()
                        || !labels
                            .iter()
                            .zip(pending.iter())
                            .all(|(actual, expected)| actual == expected)
                    {
                        return None;
                    }
                    Some((command, labels[pending.len()..].join(" ")))
                })
            })
            .collect()
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

pub(crate) fn validate_custom_command_keys(commands: &[CustomTuiCommandConfig]) -> Result<()> {
    for context in [CommandContext::Normal, CommandContext::Detail] {
        let mut assigned = COMMANDS
            .iter()
            .filter(|command| command.is_available(context))
            .flat_map(|command| {
                command
                    .keys(context)
                    .iter()
                    .map(move |key| (key.codes.to_vec(), format!(":{}", command.name)))
            })
            .collect::<Vec<_>>();
        for command in commands {
            let configured = match context {
                CommandContext::Normal => &command.keys,
                CommandContext::Detail => command.detail_keys.as_ref().unwrap_or(&command.keys),
            };
            for configured_key in configured {
                let codes = parse_key_sequence(configured_key).map_err(|reason| {
                    anyhow::anyhow!(
                        "invalid keybinding {configured_key:?} for custom command {}: {reason}",
                        command.name
                    )
                })?;
                if let Some((_, owner)) = assigned.iter().find(|(existing, _)| *existing == codes) {
                    bail!(
                        "custom command {} keybinding {} conflicts with {} in {} view",
                        command.name,
                        shortcut_label(&codes),
                        owner,
                        match context {
                            CommandContext::Normal => "list",
                            CommandContext::Detail => "detail",
                        }
                    );
                }
                assigned.push((codes, format!(":{}", command.name)));
            }
        }
    }
    Ok(())
}

fn parse_keys(keys: &[String]) -> Vec<RuntimeKeySequence> {
    keys.iter()
        .map(|key| {
            let codes = parse_key_sequence(key).expect("validated custom command keybinding");
            RuntimeKeySequence {
                label: shortcut_label(&codes),
                codes,
            }
        })
        .collect()
}

fn parse_key_sequence(input: &str) -> std::result::Result<Vec<KeyCode>, String> {
    let tokens = input.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err("sequence must not be blank".to_string());
    }
    if tokens.len() > MAX_CUSTOM_KEY_SEQUENCE_LEN {
        return Err(format!(
            "sequence may contain at most {MAX_CUSTOM_KEY_SEQUENCE_LEN} keys"
        ));
    }
    let mut codes = Vec::with_capacity(tokens.len());
    for token in tokens {
        let code = parse_key(token).ok_or_else(|| format!("unsupported key {token:?}"))?;
        if code == KeyCode::Esc || code == KeyCode::Char('?') {
            return Err(format!(
                "{} is reserved by TUI input handling",
                super::key_label(code)
            ));
        }
        if !codes.is_empty()
            && matches!(
                code,
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
            )
        {
            return Err(format!(
                "{} is reserved while a key prefix is active",
                super::key_label(code)
            ));
        }
        codes.push(code);
    }
    Ok(codes)
}

fn parse_key(token: &str) -> Option<KeyCode> {
    if token.chars().count() == 1 {
        return token.chars().next().map(KeyCode::Char);
    }
    match token.to_ascii_lowercase().as_str() {
        "space" => Some(KeyCode::Char(' ')),
        "enter" | "return" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "backspace" => Some(KeyCode::Backspace),
        "tab" => Some(KeyCode::Tab),
        "shift+tab" | "backtab" => Some(KeyCode::BackTab),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "pageup" | "page-up" => Some(KeyCode::PageUp),
        "pagedown" | "page-down" => Some(KeyCode::PageDown),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),
        _ => token
            .strip_prefix('F')
            .or_else(|| token.strip_prefix('f'))
            .and_then(|number| number.parse::<u8>().ok())
            .filter(|number| (1..=12).contains(number))
            .map(KeyCode::F),
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
            cwd: None,
            env: Default::default(),
            timeout_seconds: None,
            args: vec![],
            keys: vec!["z d".to_string()],
            detail_keys: None,
            target: CustomTuiCommandTarget::Focused,
            execution: CustomTuiCommandExecution::Wait,
            on_success: CustomTuiCommandSuccess::Quit,
        }
    }

    #[test]
    fn parses_case_sensitive_character_and_named_keys() {
        assert_eq!(
            parse_key_sequence("g D F12 Shift+Tab").unwrap(),
            vec![
                KeyCode::Char('g'),
                KeyCode::Char('D'),
                KeyCode::F(12),
                KeyCode::BackTab,
            ]
        );
        assert_eq!(
            parse_key_sequence("Space").unwrap(),
            vec![KeyCode::Char(' ')]
        );
    }

    #[test]
    fn rejects_keys_reserved_by_prefix_handling() {
        assert!(parse_key_sequence("?").is_err());
        assert!(parse_key_sequence("g Down").is_err());
        assert!(parse_key_sequence("g PageUp").is_err());
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

    #[test]
    fn custom_keybindings_resolve_in_both_contexts() {
        let catalog = CommandCatalog::new(vec![custom()]);
        for context in [CommandContext::Normal, CommandContext::Detail] {
            assert_eq!(
                catalog.resolve_shortcut(context, &[KeyCode::Char('z')]),
                CatalogShortcutLookup::Prefix
            );
            assert_eq!(
                catalog.resolve_shortcut(context, &[KeyCode::Char('z'), KeyCode::Char('d')]),
                CatalogShortcutLookup::Found(CommandHandler::Custom(0))
            );
        }
    }

    #[test]
    fn custom_target_policies_drive_scope_availability_and_shortcut_resolution() {
        let mut commands = Vec::new();
        for (index, target) in [
            CustomTuiCommandTarget::None,
            CustomTuiCommandTarget::Focused,
            CustomTuiCommandTarget::Marked,
            CustomTuiCommandTarget::MarkedOrFocused,
        ]
        .into_iter()
        .enumerate()
        {
            let mut command = custom();
            command.name = format!("command-{index}");
            command.aliases.clear();
            command.keys = vec![format!("z {index}")];
            command.target = target;
            commands.push(command);
        }
        let catalog = CommandCatalog::new(commands);
        let commands = catalog
            .commands(CommandContext::Normal)
            .into_iter()
            .filter(|command| command.is_custom())
            .collect::<Vec<_>>();

        assert_eq!(commands[0].bulk_support(), BulkSupport::NotTaskScoped);
        assert_eq!(commands[1].bulk_support(), BulkSupport::Focused);
        assert_eq!(commands[2].bulk_support(), BulkSupport::Batch);
        assert_eq!(commands[3].bulk_support(), BulkSupport::Batch);
        assert_eq!(commands[0].unavailable_reason(false, 0), None);
        assert_eq!(
            commands[1].unavailable_reason(false, 0),
            Some("requires a focused task")
        );
        assert_eq!(
            commands[2].unavailable_reason(true, 0),
            Some("requires one or more marked tasks")
        );
        assert_eq!(commands[2].unavailable_reason(false, 1), None);
        assert_eq!(
            commands[3].unavailable_reason(false, 0),
            Some("requires a marked or focused task")
        );
        assert_eq!(commands[3].unavailable_reason(true, 0), None);
        assert_eq!(commands[3].unavailable_reason(false, 1), None);
        assert_eq!(
            catalog.resolve_shortcut(
                CommandContext::Normal,
                &[KeyCode::Char('z'), KeyCode::Char('2')]
            ),
            CatalogShortcutLookup::Found(CommandHandler::Custom(2))
        );
    }

    #[test]
    fn detail_keybindings_override_inherited_keys() {
        let mut command = custom();
        command.detail_keys = Some(vec!["Z".to_string()]);
        let catalog = CommandCatalog::new(vec![command]);

        assert_eq!(
            catalog.resolve_shortcut(
                CommandContext::Normal,
                &[KeyCode::Char('z'), KeyCode::Char('d')]
            ),
            CatalogShortcutLookup::Found(CommandHandler::Custom(0))
        );
        assert_eq!(
            catalog.resolve_shortcut(CommandContext::Detail, &[KeyCode::Char('Z')]),
            CatalogShortcutLookup::Found(CommandHandler::Custom(0))
        );
        assert_eq!(
            catalog.resolve_shortcut(
                CommandContext::Detail,
                &[KeyCode::Char('z'), KeyCode::Char('d')]
            ),
            CatalogShortcutLookup::Missing
        );
    }
}

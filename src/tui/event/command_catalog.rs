use anyhow::{Result, bail};
use crossterm::event::KeyCode;

use crate::config::{CustomTuiCommandConfig, CustomTuiCommandTarget};

use super::{
    Action, BuiltInCommand, BulkSupport, COMMANDS, CommandContext, KeySequence, shortcut_label,
};

const MAX_CUSTOM_KEY_SEQUENCE_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandHandler {
    BuiltIn(Action),
    Custom(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeKeySequence {
    codes: Vec<KeyCode>,
    label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    BuiltIn(&'static BuiltInCommand),
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

    pub(crate) fn built_in(self) -> Option<&'static BuiltInCommand> {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommandCatalog {
    custom: Vec<RuntimeCustomCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogShortcutLookup {
    Found(CommandHandler),
    Prefix,
    Ambiguous(CommandHandler),
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RoutingDomain {
    Normal,
    DetailParent,
    DetailRelated,
    DetailPassive,
    DetailAttachment,
}

impl RoutingDomain {
    pub(crate) fn command_context(self) -> CommandContext {
        domain_context(self)
    }
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

    pub(crate) fn command(&self, index: usize) -> Option<CatalogCommand<'_>> {
        if let Some(command) = COMMANDS.get(index) {
            return Some(CatalogCommand::BuiltIn(command));
        }
        let id = index.checked_sub(COMMANDS.len())?;
        self.custom
            .get(id)
            .map(|command| CatalogCommand::Custom { id, command })
    }

    pub(crate) fn all_commands(&self) -> impl Iterator<Item = CatalogCommand<'_>> {
        COMMANDS.iter().map(CatalogCommand::BuiltIn).chain(
            self.custom
                .iter()
                .enumerate()
                .map(|(id, command)| CatalogCommand::Custom { id, command }),
        )
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

    pub(crate) fn resolve_shortcut_in_domain(
        &self,
        domain: RoutingDomain,
        input: &[KeyCode],
    ) -> CatalogShortcutLookup {
        self.resolve_shortcut_in_context(domain, None, input)
    }

    pub(crate) fn resolve_shortcut_in_context(
        &self,
        domain: RoutingDomain,
        section: Option<crate::tui::app::DetailSection>,
        input: &[KeyCode],
    ) -> CatalogShortcutLookup {
        if input.is_empty() {
            return CatalogShortcutLookup::Missing;
        }
        let mut exact = Vec::new();
        let mut prefix = false;
        let context = domain_context(domain);
        for command in self.commands(context) {
            if !binding_active(&command, domain, section) {
                continue;
            }
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

    pub(crate) fn prefix_hints_in_domain(
        &self,
        domain: RoutingDomain,
        pending: &[String],
    ) -> Vec<(CatalogCommand<'_>, String)> {
        let context = domain_context(domain);
        self.commands(context)
            .into_iter()
            .filter(|command| binding_active(command, domain, None))
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
}

pub(crate) fn validate_custom_command_keys(commands: &[CustomTuiCommandConfig]) -> Result<()> {
    const DOMAINS: &[RoutingDomain] = &[
        RoutingDomain::Normal,
        RoutingDomain::DetailParent,
        RoutingDomain::DetailRelated,
        RoutingDomain::DetailPassive,
        RoutingDomain::DetailAttachment,
    ];
    for &domain in DOMAINS {
        let context = domain_context(domain);
        let mut assigned = Vec::<(Vec<KeyCode>, String)>::new();
        for command in COMMANDS
            .iter()
            .map(CatalogCommand::BuiltIn)
            .filter(|command| binding_active(command, domain, None))
        {
            for key in command.keys(context) {
                if let Some((_, owner)) = assigned
                    .iter()
                    .find(|(existing, _)| existing.as_slice() == key.codes)
                {
                    bail!(
                        "built-in keybinding {} conflicts between {} and :{} in {} routing domain",
                        shortcut_label(key.codes),
                        owner,
                        command.name(),
                        routing_domain_name(domain),
                    );
                }
                assigned.push((key.codes.to_vec(), format!(":{}", command.name())));
            }
        }
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
                        "custom command {} keybinding {} conflicts with {} in {} routing domain",
                        command.name,
                        shortcut_label(&codes),
                        owner,
                        routing_domain_name(domain),
                    );
                }
                assigned.push((codes, format!(":{}", command.name)));
            }
        }
    }
    Ok(())
}

fn routing_domain_name(domain: RoutingDomain) -> &'static str {
    match domain {
        RoutingDomain::Normal => "normal",
        RoutingDomain::DetailParent => "detail parent",
        RoutingDomain::DetailRelated => "detail related",
        RoutingDomain::DetailPassive => "detail passive",
        RoutingDomain::DetailAttachment => "detail attachment",
    }
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

pub(super) fn domain_context(domain: RoutingDomain) -> CommandContext {
    match domain {
        RoutingDomain::DetailParent
        | RoutingDomain::DetailRelated
        | RoutingDomain::DetailPassive
        | RoutingDomain::DetailAttachment => CommandContext::Detail,
        _ => CommandContext::Normal,
    }
}

pub(crate) fn focus_policy_compatible(
    policy: super::DetailFocusPolicy,
    domain: RoutingDomain,
    section: Option<crate::tui::app::DetailSection>,
) -> bool {
    use super::DetailFocusPolicy;
    match domain {
        RoutingDomain::DetailAttachment => {
            matches!(
                policy,
                DetailFocusPolicy::Global | DetailFocusPolicy::Attachment
            )
        }
        RoutingDomain::DetailPassive => policy == DetailFocusPolicy::Global,
        RoutingDomain::DetailRelated => match policy {
            DetailFocusPolicy::Global | DetailFocusPolicy::RelatedTask => true,
            DetailFocusPolicy::EpicChild => section
                .is_none_or(|section| section == crate::tui::app::DetailSection::EpicChildren),
            _ => false,
        },
        RoutingDomain::DetailParent | RoutingDomain::Normal => {
            policy != DetailFocusPolicy::Attachment
        }
    }
}

pub(crate) fn binding_active(
    command: &CatalogCommand<'_>,
    domain: RoutingDomain,
    section: Option<crate::tui::app::DetailSection>,
) -> bool {
    let Some(command) = command.built_in() else {
        return true;
    };
    focus_policy_compatible(command.detail_focus(), domain, section)
        || (command.action == Action::RemoveEpicChild
            && domain == RoutingDomain::DetailRelated
            && section == Some(crate::tui::app::DetailSection::EpicParent))
}

pub(crate) fn command_match_rank(command: CatalogCommand<'_>, input: &str) -> Option<u8> {
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
    } else if command.description().starts_with(input) {
        Some(4)
    } else if command.description().split_whitespace().any(|word| {
        word.trim_matches(|character: char| !character.is_alphanumeric())
            .starts_with(input)
    }) {
        Some(5)
    } else if command.description().contains(input) {
        Some(6)
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
    use crate::tui::event::CommandQuery;
    use crate::tui::event::command_query::CommandAvailability;

    fn snapshot(
        surface: super::super::CommandSurfaceSnapshot,
        recurrence_series_id: Option<aven_core::recurrence::RecurrenceSeriesId>,
    ) -> super::super::CommandSessionSnapshot {
        let workspace = crate::workspaces::Workspace::default();
        super::super::CommandSessionSnapshot {
            workspace: super::super::CommandWorkspaceSnapshot {
                id: workspace.id,
                key: workspace.key,
                name: workspace.name,
            },
            surface,
            recurrence_series_id,
        }
    }

    fn resolve_catalog_shortcut(
        catalog: &CommandCatalog,
        context: CommandContext,
        input: &[KeyCode],
    ) -> CatalogShortcutLookup {
        let domain = match context {
            CommandContext::Normal => RoutingDomain::Normal,
            CommandContext::Detail => RoutingDomain::DetailParent,
        };
        catalog.resolve_shortcut_in_domain(domain, input)
    }

    fn query_names(
        catalog: &CommandCatalog,
        snapshot: &super::super::CommandSessionSnapshot,
        input: &str,
    ) -> Vec<String> {
        catalog
            .query(CommandQuery {
                input,
                snapshot,
                unavailable: &[],
            })
            .into_iter()
            .filter_map(|candidate| catalog.command(candidate.index))
            .map(|command| command.name().to_string())
            .collect()
    }

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
        let snapshot = snapshot(super::super::CommandSurfaceSnapshot::AddTaskOnly, None);
        let canonical = query_names(&catalog, &snapshot, "dispatch");
        let alias = query_names(&catalog, &snapshot, "custom-dispatch");

        assert_eq!(canonical, vec!["dispatch"]);
        assert_eq!(alias, canonical);
    }

    #[test]
    fn session_query_includes_custom_and_built_in_commands() {
        let catalog = CommandCatalog::new(vec![custom()]);
        let task_id = crate::test_support::task_id("catalog-query-task");
        let snapshot = snapshot(
            super::super::CommandSurfaceSnapshot::List {
                primary_task_id: Some(task_id.clone()),
                marked_task_ids: vec![],
                visible_task_ids: vec![task_id.clone()],
                focused_sidebar: None,
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        let names = query_names(&catalog, &snapshot, "d");

        assert!(names.contains(&"dispatch".to_string()));
        assert!(names.contains(&"delete".to_string()));
        assert_eq!(
            query_names(&catalog, &snapshot, "custom-d"),
            vec!["dispatch"]
        );
    }

    #[test]
    fn custom_keybindings_resolve_in_both_contexts() {
        let catalog = CommandCatalog::new(vec![custom()]);
        for context in [CommandContext::Normal, CommandContext::Detail] {
            assert_eq!(
                resolve_catalog_shortcut(&catalog, context, &[KeyCode::Char('z')]),
                CatalogShortcutLookup::Prefix
            );
            assert_eq!(
                resolve_catalog_shortcut(
                    &catalog,
                    context,
                    &[KeyCode::Char('z'), KeyCode::Char('d')]
                ),
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
            resolve_catalog_shortcut(
                &catalog,
                CommandContext::Normal,
                &[KeyCode::Char('z'), KeyCode::Char('2')]
            ),
            CatalogShortcutLookup::Found(CommandHandler::Custom(2))
        );
    }

    #[test]
    fn contextual_first_pages_are_explicit_and_deterministic() {
        use super::super::{CommandSurfaceSnapshot, DetailCommandFocus, SidebarCommandTarget};
        use crate::tui::app::DetailSection;
        use crate::tui::store::TaskQuery;

        let catalog = CommandCatalog::default();
        let parent_id = crate::test_support::task_id("command-parent");
        let related_id = crate::test_support::task_id("command-related");
        let parent = snapshot(
            CommandSurfaceSnapshot::Detail {
                parent_task_id: parent_id.clone(),
                marked_task_ids: Vec::new(),
                focus: None,
                scroll: 0,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &parent, "")[..8],
            &[
                "edit-title",
                "edit-description",
                "status-picker",
                "edit-priority",
                "edit-project",
                "edit-labels",
                "add-note",
                "add-dependency"
            ]
        );

        let marked = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: Some(parent_id.clone()),
                marked_task_ids: vec![parent_id.clone(), related_id.clone()],
                visible_task_ids: vec![parent_id.clone(), related_id.clone()],
                focused_sidebar: None,
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &marked, "")[..8],
            &[
                "status-picker",
                "status-done",
                "edit-project",
                "edit-priority",
                "edit-labels",
                "delete",
                "copy-ref",
                "clear-marks"
            ]
        );

        let attachment = snapshot(
            CommandSurfaceSnapshot::Detail {
                parent_task_id: parent_id.clone(),
                marked_task_ids: Vec::new(),
                focus: Some(DetailCommandFocus::Attachment {
                    attachment_id: "ATTACHMENT000001".to_string(),
                    bytes_present: true,
                }),
                scroll: 0,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &attachment, "")[..3],
            &["attachment-open", "attachment-save", "attachment-delete"]
        );

        let epic_child = snapshot(
            CommandSurfaceSnapshot::Detail {
                parent_task_id: parent_id.clone(),
                marked_task_ids: Vec::new(),
                focus: Some(DetailCommandFocus::Relationship {
                    section: DetailSection::EpicChildren,
                    task_id: related_id.clone(),
                }),
                scroll: 0,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &epic_child, "")[..6],
            &[
                "status-picker",
                "status-done",
                "edit-title",
                "copy-ref",
                "copy-title",
                "task-child-remove"
            ]
        );

        let note = snapshot(
            CommandSurfaceSnapshot::Detail {
                parent_task_id: parent_id.clone(),
                marked_task_ids: Vec::new(),
                focus: Some(DetailCommandFocus::Note),
                scroll: 0,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &note, "")[..4],
            &["back", "search", "refresh", "command"]
        );

        let disclosure = snapshot(
            CommandSurfaceSnapshot::Detail {
                parent_task_id: parent_id.clone(),
                marked_task_ids: Vec::new(),
                focus: Some(DetailCommandFocus::Disclosure),
                scroll: 0,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &disclosure, "")[..4],
            &["back", "search", "refresh", "command"]
        );

        let sidebar = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: Some(parent_id.clone()),
                marked_task_ids: vec![],
                visible_task_ids: vec![parent_id.clone()],
                focused_sidebar: Some(SidebarCommandTarget::View(TaskQuery::Queue)),
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        assert_eq!(query_names(&catalog, &sidebar, "")[0], "view-queue");

        let sidebar_workspace = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: Some(parent_id.clone()),
                marked_task_ids: vec![],
                visible_task_ids: vec![parent_id.clone()],
                focused_sidebar: Some(SidebarCommandTarget::Workspace),
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &sidebar_workspace, "")[..4],
            &[
                "scope-all",
                "workspace-switch",
                "workspace-rename",
                "workspace-create"
            ]
        );

        let sidebar_project = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: Some(parent_id.clone()),
                marked_task_ids: vec![],
                visible_task_ids: vec![parent_id.clone()],
                focused_sidebar: Some(SidebarCommandTarget::Project("aven".to_string())),
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &sidebar_project, "")[..4],
            &[
                "scope-project",
                "rename-project",
                "add-task",
                "add-project-path"
            ]
        );

        let selected = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: Some(parent_id.clone()),
                marked_task_ids: vec![],
                visible_task_ids: vec![parent_id.clone()],
                focused_sidebar: None,
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &selected, "")[..4],
            &[
                "status-picker",
                "status-done",
                "edit-title",
                "edit-priority"
            ]
        );

        let empty = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: None,
                marked_task_ids: vec![],
                visible_task_ids: vec![],
                focused_sidebar: None,
                is_empty: true,
                empty_preferred_action: Some(Action::BeginSearch),
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &empty, "")[..4],
            &["search", "add-task", "view-queue", "filter-clear"]
        );
        let clear_filter_empty = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: None,
                marked_task_ids: vec![],
                visible_task_ids: vec![],
                focused_sidebar: None,
                is_empty: true,
                empty_preferred_action: Some(Action::ClearFilters),
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &clear_filter_empty, "")[..3],
            &["filter-clear", "add-task", "search"]
        );
        let deleted_empty = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: None,
                marked_task_ids: vec![],
                visible_task_ids: vec![],
                focused_sidebar: None,
                is_empty: true,
                empty_preferred_action: Some(Action::ToggleDeletedFilter),
            },
            None,
        );
        assert_eq!(
            query_names(&catalog, &deleted_empty, "")[0],
            "filter-deleted"
        );

        let neutral = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: None,
                marked_task_ids: vec![],
                visible_task_ids: vec![],
                focused_sidebar: None,
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &neutral, "")[..4],
            &["add-task", "search", "view-queue", "scope-project"]
        );

        let add_task_only = snapshot(CommandSurfaceSnapshot::AddTaskOnly, None);
        assert_eq!(
            query_names(&catalog, &add_task_only, ""),
            &["add-task", "quit", "help", "config-show"]
        );

        let series_id = aven_core::recurrence::RecurrenceSeriesId::new();
        let recurring = snapshot(
            CommandSurfaceSnapshot::RecurrenceList {
                focused_sidebar: None,
                is_empty: false,
                empty_preferred_action: None,
            },
            Some(series_id),
        );
        assert_eq!(
            &query_names(&catalog, &recurring, "")[..4],
            &[
                "recurrence-edit-template",
                "recurrence-pause",
                "recurrence-resume",
                "recurrence-stop"
            ]
        );

        let relationship = snapshot(
            CommandSurfaceSnapshot::Detail {
                parent_task_id: parent_id.clone(),
                marked_task_ids: Vec::new(),
                focus: Some(DetailCommandFocus::Relationship {
                    section: DetailSection::DependsOn,
                    task_id: related_id,
                }),
                scroll: 0,
            },
            None,
        );
        assert_eq!(
            &query_names(&catalog, &relationship, "")[..5],
            &[
                "status-picker",
                "status-done",
                "edit-title",
                "copy-ref",
                "copy-title"
            ]
        );
    }

    #[test]
    fn text_tiers_precede_availability_and_context() {
        use super::super::{CommandSurfaceSnapshot, DetailCommandFocus};

        let parent_id = crate::test_support::task_id("text-tier-parent");
        let snapshot = snapshot(
            CommandSurfaceSnapshot::Detail {
                parent_task_id: parent_id.clone(),
                marked_task_ids: Vec::new(),
                focus: Some(DetailCommandFocus::Attachment {
                    attachment_id: "ATTACHMENT000002".to_string(),
                    bytes_present: true,
                }),
                scroll: 0,
            },
            None,
        );
        let catalog = CommandCatalog::default();
        let matches = catalog.query(CommandQuery {
            input: "delete",
            snapshot: &snapshot,
            unavailable: &[],
        });

        assert_eq!(catalog.command(matches[0].index).unwrap().name(), "delete");
        assert!(matches[0].availability.reason().is_some());
        assert!(matches.iter().any(|candidate| {
            catalog.command(candidate.index).unwrap().name() == "attachment-delete"
                && candidate.availability == CommandAvailability::Ready
        }));
    }

    #[test]
    fn typed_queries_keep_commands_unavailable_in_the_captured_context() {
        use super::super::CommandSurfaceSnapshot;

        let task_id = crate::test_support::task_id("typed-disabled-target");
        let snapshot = snapshot(
            CommandSurfaceSnapshot::List {
                primary_task_id: Some(task_id.clone()),
                marked_task_ids: vec![],
                visible_task_ids: vec![task_id.clone()],
                focused_sidebar: None,
                is_empty: false,
                empty_preferred_action: None,
            },
            None,
        );
        let catalog = CommandCatalog::default();
        assert!(
            query_names(&catalog, &snapshot, "")
                .iter()
                .all(|name| name != "attachment-save")
        );
        let matches = catalog.query(CommandQuery {
            input: "attachment-save",
            snapshot: &snapshot,
            unavailable: &[],
        });
        assert_eq!(
            catalog.command(matches[0].index).unwrap().name(),
            "attachment-save"
        );
        assert_eq!(
            matches[0].availability.reason(),
            Some("requires a focused attachment")
        );
        let done = query_names(&catalog, &snapshot, "done");
        assert!(done.len() > 1);
        assert!(done.iter().any(|name| name == "status-done"));
    }

    #[test]
    fn attachment_domain_routes_status_and_save_without_conflict() {
        let catalog = CommandCatalog::default();
        assert_eq!(
            catalog.resolve_shortcut_in_domain(RoutingDomain::DetailParent, &[KeyCode::Char('s')]),
            CatalogShortcutLookup::Found(CommandHandler::BuiltIn(Action::BeginStatusPicker))
        );
        assert_eq!(
            catalog.resolve_shortcut_in_domain(RoutingDomain::DetailRelated, &[KeyCode::Char('s')]),
            CatalogShortcutLookup::Found(CommandHandler::BuiltIn(Action::BeginStatusPicker))
        );
        assert_eq!(
            catalog
                .resolve_shortcut_in_domain(RoutingDomain::DetailAttachment, &[KeyCode::Char('s')]),
            CatalogShortcutLookup::Found(CommandHandler::BuiltIn(Action::SaveAttachment))
        );
    }

    #[test]
    fn routing_domains_filter_prefix_hints_and_preserve_exact_prefix_precedence() {
        let catalog = CommandCatalog::default();
        assert!(
            catalog
                .prefix_hints_in_domain(RoutingDomain::DetailAttachment, &["t".to_string()])
                .is_empty()
        );
        assert!(
            catalog
                .prefix_hints_in_domain(RoutingDomain::DetailRelated, &["t".to_string()])
                .iter()
                .any(|(command, _)| command.name() == "remove-dependency")
        );

        let mut short = custom();
        short.name = "short".to_string();
        short.keys = vec!["z".to_string()];
        let mut long = custom();
        long.name = "long".to_string();
        long.keys = vec!["z x".to_string()];
        validate_custom_command_keys(&[short.clone(), long.clone()]).unwrap();
        let catalog = CommandCatalog::new(vec![short, long]);
        assert_eq!(
            catalog.resolve_shortcut_in_domain(RoutingDomain::Normal, &[KeyCode::Char('z')]),
            CatalogShortcutLookup::Ambiguous(CommandHandler::Custom(0))
        );
        assert_eq!(
            catalog.resolve_shortcut_in_domain(
                RoutingDomain::Normal,
                &[KeyCode::Char('z'), KeyCode::Char('x')]
            ),
            CatalogShortcutLookup::Found(CommandHandler::Custom(1))
        );
    }

    #[test]
    fn detail_keybindings_override_inherited_keys() {
        let mut command = custom();
        command.detail_keys = Some(vec!["Z".to_string()]);
        let catalog = CommandCatalog::new(vec![command]);

        assert_eq!(
            resolve_catalog_shortcut(
                &catalog,
                CommandContext::Normal,
                &[KeyCode::Char('z'), KeyCode::Char('d')]
            ),
            CatalogShortcutLookup::Found(CommandHandler::Custom(0))
        );
        assert_eq!(
            resolve_catalog_shortcut(&catalog, CommandContext::Detail, &[KeyCode::Char('Z')]),
            CatalogShortcutLookup::Found(CommandHandler::Custom(0))
        );
        assert_eq!(
            resolve_catalog_shortcut(
                &catalog,
                CommandContext::Detail,
                &[KeyCode::Char('z'), KeyCode::Char('d')]
            ),
            CatalogShortcutLookup::Missing
        );
    }
}

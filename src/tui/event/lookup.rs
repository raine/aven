use crossterm::event::KeyCode;
use unicode_width::UnicodeWidthStr;

use super::{Action, BulkSupport, CommandContext, CommandSpec, KeySequence};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandLookup {
    Empty,
    Found(Action),
    Ambiguous,
    Missing,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CommandSpecLookup {
    Empty,
    Found(&'static CommandSpec),
    Ambiguous,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandCompletion {
    Empty,
    Missing,
    Unchanged,
    Completed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShortcutLookup {
    Found(Action),
    Prefix,
    Ambiguous(Action),
    Missing,
}

pub(crate) fn key_label(code: KeyCode) -> String {
    match code {
        KeyCode::Char(ch) => ch.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        _ => format!("{code:?}"),
    }
}

pub(crate) fn shortcut_label(codes: &[KeyCode]) -> String {
    codes
        .iter()
        .map(|code| key_label(*code))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn preferred_shortcut_label(
    action: Action,
    context: CommandContext,
) -> Option<&'static str> {
    context
        .commands()
        .filter(|command| command.action == action)
        .flat_map(|command| command.keys(context))
        .min_by_key(|key| (key.codes.len(), UnicodeWidthStr::width(key.label)))
        .map(|key| key.label)
}

pub(crate) fn resolve_shortcut(input: &[KeyCode]) -> ShortcutLookup {
    resolve_shortcut_for(CommandContext::Normal, input)
}

pub(crate) fn resolve_shortcut_for(context: CommandContext, input: &[KeyCode]) -> ShortcutLookup {
    resolve_shortcut_iter(context.commands(), context, input)
}

#[allow(dead_code)]
pub(crate) fn resolve_shortcut_in(commands: &[CommandSpec], input: &[KeyCode]) -> ShortcutLookup {
    resolve_shortcut_in_for(commands, CommandContext::Normal, input)
}

#[allow(dead_code)]
pub(crate) fn resolve_shortcut_in_for(
    commands: &[CommandSpec],
    context: CommandContext,
    input: &[KeyCode],
) -> ShortcutLookup {
    resolve_shortcut_iter(commands.iter(), context, input)
}

fn resolve_shortcut_iter<'a>(
    commands: impl IntoIterator<Item = &'a CommandSpec>,
    context: CommandContext,
    input: &[KeyCode],
) -> ShortcutLookup {
    if input.is_empty() {
        return ShortcutLookup::Missing;
    }

    let mut exact = Vec::new();
    let mut prefix = false;

    for command in commands {
        for key in command.keys(context) {
            if key.codes == input {
                exact.push(command.action);
            } else if key.codes.starts_with(input) {
                prefix = true;
            }
        }
    }

    match (exact.as_slice(), prefix) {
        ([action], false) => ShortcutLookup::Found(*action),
        ([action], true) => ShortcutLookup::Ambiguous(*action),
        ([action, ..], _) => ShortcutLookup::Ambiguous(*action),
        ([], true) => ShortcutLookup::Prefix,
        ([], false) => ShortcutLookup::Missing,
    }
}

#[allow(dead_code)]
pub(crate) fn matching_commands(input: &str) -> Vec<&'static CommandSpec> {
    matching_commands_for(CommandContext::Normal, input)
}

pub(crate) fn matching_commands_for(
    context: CommandContext,
    input: &str,
) -> Vec<&'static CommandSpec> {
    let input = normalize_command_input(input);
    if input.is_empty() {
        return context.commands().collect();
    }
    let mut matches = context
        .commands()
        .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
        .collect::<Vec<_>>();
    matches.sort_by_key(|(rank, _)| *rank);
    matches.into_iter().map(|(_, command)| command).collect()
}

pub(crate) fn matching_commands_for_bulk(
    context: CommandContext,
    input: &str,
    marked_task_count: usize,
) -> Vec<&'static CommandSpec> {
    let mut matches = matching_commands_for(context, input);
    if marked_task_count == 0 || !normalize_command_input(input).is_empty() {
        return matches;
    }
    matches.sort_by_key(|command| match command.bulk_support() {
        BulkSupport::Batch => 0,
        BulkSupport::BulkControl => 1,
        BulkSupport::Focused => 2,
        BulkSupport::NotTaskScoped => 3,
        BulkSupport::SingleOnly(_) => 4,
    });
    matches
}

fn normalize_command_input(input: &str) -> &str {
    input.trim().strip_prefix(':').unwrap_or(input.trim())
}

fn command_match_rank(command: &CommandSpec, input: &str) -> Option<u8> {
    if command.name == input || command.aliases.contains(&input) {
        Some(0)
    } else if command.name.starts_with(input)
        || command.aliases.iter().any(|alias| alias.starts_with(input))
        || dashless_eq(command.name, input)
        || command
            .aliases
            .iter()
            .any(|alias| dashless_eq(alias, input))
    {
        Some(1)
    } else if dashless_starts_with(command.name, input)
        || command
            .aliases
            .iter()
            .any(|alias| dashless_starts_with(alias, input))
    {
        Some(2)
    } else if command
        .name
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
    if !value.contains('-') {
        return false;
    }
    let dashless = value.chars().filter(|ch| *ch != '-').collect::<String>();
    dashless == input
}

fn dashless_starts_with(value: &str, input: &str) -> bool {
    if !value.contains('-') {
        return false;
    }
    let dashless = value.chars().filter(|ch| *ch != '-').collect::<String>();
    dashless.starts_with(input)
}

#[allow(dead_code)]
pub(crate) fn complete_command(input: &str) -> CommandCompletion {
    complete_command_for(CommandContext::Normal, input)
}

pub(crate) fn complete_command_for(context: CommandContext, input: &str) -> CommandCompletion {
    let input = normalize_command_input(input);
    if input.is_empty() {
        return CommandCompletion::Empty;
    }
    let names = best_match_names(context, input);
    if names.is_empty() {
        return CommandCompletion::Missing;
    }
    if names.len() > 1 {
        return CommandCompletion::Unchanged;
    }
    let completion = names[0].to_string();
    if completion.len() > input.len() {
        CommandCompletion::Completed(completion)
    } else {
        CommandCompletion::Unchanged
    }
}

#[allow(dead_code)]
pub(crate) fn command_cycle_options(input: &str) -> Vec<&'static str> {
    command_cycle_options_for(CommandContext::Normal, input)
}

pub(crate) fn command_cycle_options_for(context: CommandContext, input: &str) -> Vec<&'static str> {
    let input = normalize_command_input(input);
    if input.is_empty() {
        return Vec::new();
    }
    ranked_matches(context, input)
        .into_iter()
        .map(|(_, command)| command.name)
        .collect()
}

fn best_match_names(context: CommandContext, input: &str) -> Vec<&'static str> {
    let matches = ranked_matches(context, input);
    let Some(best_rank) = matches.iter().map(|(rank, _)| *rank).min() else {
        return Vec::new();
    };
    matches
        .iter()
        .filter(|(rank, _)| *rank == best_rank)
        .map(|(_, command)| command.name)
        .collect()
}

fn ranked_matches(context: CommandContext, input: &str) -> Vec<(u8, &'static CommandSpec)> {
    let mut matches = context
        .commands()
        .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
        .collect::<Vec<_>>();
    matches.sort_by_key(|(rank, _)| *rank);
    matches
}

pub(crate) fn prefix_hint_commands(
    context: CommandContext,
    pending: &[String],
) -> Vec<(&'static CommandSpec, &'static KeySequence, String)> {
    context
        .commands()
        .flat_map(|command| {
            command.keys(context).iter().filter_map(move |key| {
                if key.codes.len() <= pending.len() {
                    return None;
                }
                let labels = key
                    .codes
                    .iter()
                    .map(|code| key_label(*code))
                    .collect::<Vec<_>>();
                if labels.len() <= pending.len()
                    || !labels
                        .iter()
                        .zip(pending.iter())
                        .all(|(actual, expected)| actual == expected)
                {
                    return None;
                }
                Some((command, key, labels[pending.len()..].join(" ")))
            })
        })
        .collect()
}

#[allow(dead_code)]
pub(crate) fn lookup_command_spec(input: &str) -> CommandSpecLookup {
    lookup_command_spec_for(CommandContext::Normal, input)
}

pub(crate) fn lookup_command_spec_for(context: CommandContext, input: &str) -> CommandSpecLookup {
    let input = normalize_command_input(input);
    if input.is_empty() {
        return CommandSpecLookup::Empty;
    }
    let matches = context
        .commands()
        .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
        .collect::<Vec<_>>();
    let Some(best_rank) = matches.iter().map(|(rank, _)| *rank).min() else {
        return CommandSpecLookup::Missing;
    };
    let mut best_matches = matches
        .into_iter()
        .filter(|(rank, _)| *rank == best_rank)
        .map(|(_, command)| command);
    let Some(command) = best_matches.next() else {
        return CommandSpecLookup::Missing;
    };
    if best_matches.next().is_some() {
        CommandSpecLookup::Ambiguous
    } else {
        CommandSpecLookup::Found(command)
    }
}

#[allow(dead_code)]
pub(crate) fn lookup_command(input: &str) -> CommandLookup {
    let input = normalize_command_input(input);
    if input.is_empty() {
        return CommandLookup::Empty;
    }
    let matches = CommandContext::Normal
        .commands()
        .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
        .collect::<Vec<_>>();
    let Some(best_rank) = matches.iter().map(|(rank, _)| *rank).min() else {
        return CommandLookup::Missing;
    };
    let mut best_matches = matches
        .into_iter()
        .filter(|(rank, _)| *rank == best_rank)
        .map(|(_, command)| command);
    let Some(command) = best_matches.next() else {
        return CommandLookup::Missing;
    };
    if best_matches.next().is_some() {
        CommandLookup::Ambiguous
    } else {
        CommandLookup::Found(command.action)
    }
}

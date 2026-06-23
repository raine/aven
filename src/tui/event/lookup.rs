use crossterm::event::KeyCode;

use super::{Action, COMMANDS, CommandContext, CommandSpec, KeySequence};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandLookup {
    Empty,
    Found(Action),
    Ambiguous,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

pub(crate) fn resolve_shortcut(input: &[KeyCode]) -> ShortcutLookup {
    resolve_shortcut_for(CommandContext::Normal, input)
}

pub(crate) fn resolve_shortcut_for(context: CommandContext, input: &[KeyCode]) -> ShortcutLookup {
    resolve_shortcut_in(context.commands(), input)
}

pub(crate) fn resolve_shortcut_in(commands: &[CommandSpec], input: &[KeyCode]) -> ShortcutLookup {
    if input.is_empty() {
        return ShortcutLookup::Missing;
    }

    let mut exact = Vec::new();
    let mut prefix = false;

    for command in commands {
        for key in command.keys {
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

pub(crate) fn matching_commands(input: &str) -> Vec<&'static CommandSpec> {
    let input = normalize_command_input(input);
    if input.is_empty() {
        return COMMANDS.iter().collect();
    }
    let mut matches = COMMANDS
        .iter()
        .filter_map(|command| command_match_rank(command, input).map(|rank| (rank, command)))
        .collect::<Vec<_>>();
    matches.sort_by_key(|(rank, _)| *rank);
    matches.into_iter().map(|(_, command)| command).collect()
}

fn normalize_command_input(input: &str) -> &str {
    input.trim().strip_prefix(':').unwrap_or(input.trim())
}

fn command_match_rank(command: &CommandSpec, input: &str) -> Option<u8> {
    if command.name == input || command.aliases.contains(&input) {
        Some(0)
    } else if command.name.starts_with(input)
        || command.aliases.iter().any(|alias| alias.starts_with(input))
    {
        Some(1)
    } else if command
        .name
        .split('-')
        .skip(1)
        .any(|segment| segment.starts_with(input))
    {
        Some(2)
    } else {
        None
    }
}

pub(crate) fn prefix_hint_commands(
    context: CommandContext,
    pending: &[String],
) -> Vec<(&'static CommandSpec, &'static KeySequence, String)> {
    context
        .commands()
        .iter()
        .flat_map(|command| {
            command.keys.iter().filter_map(move |key| {
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
                Some((command, key, labels[pending.len()].clone()))
            })
        })
        .collect()
}

pub(crate) fn lookup_command(input: &str) -> CommandLookup {
    let input = normalize_command_input(input);
    if input.is_empty() {
        return CommandLookup::Empty;
    }
    let matches = COMMANDS
        .iter()
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

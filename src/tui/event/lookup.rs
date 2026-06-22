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
    let input = input.trim();
    if input.is_empty() {
        return COMMANDS.iter().collect();
    }
    COMMANDS
        .iter()
        .filter(|command| command.name == input || command.name.starts_with(input))
        .collect()
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
    let input = input.trim();
    if input.is_empty() {
        return CommandLookup::Empty;
    }
    let matches = matching_commands(input);
    match matches.as_slice() {
        [command] => CommandLookup::Found(command.action),
        [] => CommandLookup::Missing,
        _ => CommandLookup::Ambiguous,
    }
}

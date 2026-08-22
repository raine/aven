use crossterm::event::KeyCode;
use unicode_width::UnicodeWidthStr;

use super::{Action, BuiltInCommand, CommandContext, KeySequence};

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

#[allow(dead_code)]
pub(crate) fn resolve_shortcut_for(context: CommandContext, input: &[KeyCode]) -> ShortcutLookup {
    let domain = match context {
        CommandContext::Normal => super::RoutingDomain::Normal,
        CommandContext::Detail => super::RoutingDomain::DetailParent,
    };
    match super::CommandCatalog::default().resolve_shortcut_in_domain(domain, input) {
        super::CatalogShortcutLookup::Found(super::CommandHandler::BuiltIn(action)) => {
            ShortcutLookup::Found(action)
        }
        super::CatalogShortcutLookup::Ambiguous(super::CommandHandler::BuiltIn(action)) => {
            ShortcutLookup::Ambiguous(action)
        }
        super::CatalogShortcutLookup::Prefix => ShortcutLookup::Prefix,
        super::CatalogShortcutLookup::Missing => ShortcutLookup::Missing,
        super::CatalogShortcutLookup::Found(super::CommandHandler::Custom(_))
        | super::CatalogShortcutLookup::Ambiguous(super::CommandHandler::Custom(_)) => {
            unreachable!("default catalog has no custom commands")
        }
    }
}

#[allow(dead_code)]
pub(crate) fn resolve_shortcut_in_for(
    commands: &[BuiltInCommand],
    context: CommandContext,
    input: &[KeyCode],
) -> ShortcutLookup {
    resolve_shortcut_iter(commands.iter(), context, input)
}

fn resolve_shortcut_iter<'a>(
    commands: impl IntoIterator<Item = &'a BuiltInCommand>,
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
pub(crate) fn prefix_hint_commands(
    context: CommandContext,
    pending: &[String],
) -> Vec<(&'static BuiltInCommand, &'static KeySequence, String)> {
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

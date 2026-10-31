//! The capability map: every prototype user action, its core or host operation,
//! owning phase, acceptance and scenes. Demo controls are marked test-only.
use std::collections::BTreeSet;

use anyhow::{Result, bail};
use serde::Deserialize;

pub const PHASES: &[&str] = &[
    "foundation",
    "core-screens",
    "workflows",
    "settings-sync",
    "desktop-finish",
    "integration",
];
/// Actions with no inventory entry, such as pointer gestures, use this prefix.
const GESTURE_PREFIX: &str = "gesture:";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityMap {
    pub capability: Vec<Capability>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    /// `act:`, `chord:`, `key:`, `composer:`, `palette:`, `dialog:`, `field:`,
    /// `listener:`, `handler-key:`, `attribute:` or `gesture:`.
    pub id: String,
    pub action: String,
    /// The `aven-core` or host operation behind the action.
    pub operation: String,
    /// Empty only for test-only demo controls.
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub scenes: Vec<String>,
    #[serde(default)]
    pub test_only: bool,
}

impl CapabilityMap {
    pub fn parse(text: &str) -> Result<Self> {
        let map: Self = toml::from_str(text)?;
        map.validate()?;
        Ok(map)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for entry in &self.capability {
            if !seen.insert(entry.id.as_str()) {
                bail!("{} is listed twice", entry.id);
            }
            if entry.test_only {
                if !entry.phase.is_empty() {
                    bail!("{} is test-only and cannot have an owner", entry.id);
                }
            } else if !PHASES.contains(&entry.phase.as_str()) {
                bail!("{} has unknown phase {:?}", entry.id, entry.phase);
            } else if entry.operation.is_empty()
                || entry.acceptance.is_empty()
                || entry.scenes.is_empty()
            {
                bail!("{} needs an operation, acceptance and scenes", entry.id);
            }
        }
        Ok(())
    }

    /// Fails on prototype actions with no entry and on entries the prototype
    /// no longer has.
    pub fn check_coverage(&self, inventory: &BTreeSet<String>) -> Result<()> {
        let mapped: BTreeSet<_> = self
            .capability
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        let missing: Vec<_> = inventory.difference(&mapped).collect();
        let stale: Vec<_> = mapped
            .difference(inventory)
            .filter(|id| !id.starts_with(GESTURE_PREFIX))
            .collect();
        if !missing.is_empty() || !stale.is_empty() {
            bail!("unmapped prototype actions: {missing:?}; stale entries: {stale:?}");
        }
        Ok(())
    }
}

/// What `source_surfaces` cannot see: text a user types into a field, native
/// browser behaviour, and handlers built from computed names.
pub const NOT_INVENTORIED: &str = "typed text, browser defaults and computed handler names";

/// Data attributes that style or carry values rather than dispatch an action.
const PASSIVE_ATTRIBUTES: &[&str] = &["a", "c", "d", "v", "qa", "qd"];

/// Controls wired outside the action tables, read from the page's script line
/// by line. Each id names the top-level function that owns the control, because
/// names such as `ok` repeat across dialogs:
/// `dialog:` for `data-d`, `data-qa` and `data-qd` buttons, `field:` for inputs
/// with an id, `listener:` for assigned and added handlers, `handler-key:` for
/// keys read in any handler outside the main keymap between `keymap_start` and
/// `keymap_end`, and `attribute:` for other data attributes.
pub fn source_surfaces(script: &str, keymap_start: &str, keymap_end: &str) -> BTreeSet<String> {
    let mut surfaces = BTreeSet::new();
    let mut owner = "page".to_string();
    let mut in_keymap = false;
    for line in script.lines() {
        if let Some(name) = owner_of(line) {
            owner = name;
        }
        in_keymap = (in_keymap || line.contains(keymap_start)) && !line.contains(keymap_end);
        dialog_controls(line, &owner, &mut surfaces);
        fields(line, &mut surfaces);
        listeners(line, &owner, &mut surfaces);
        attributes(line, &mut surfaces);
        if !in_keymap {
            handler_keys(line, &owner, &mut surfaces);
        }
    }
    surfaces
}

fn identifier(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(text.len());
    &text[..end]
}

/// The name a line at the left margin opens, if any.
fn owner_of(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("async function ")
        .or_else(|| line.strip_prefix("function "))
        .or_else(|| line.strip_prefix("const "))
        .or_else(|| line.strip_prefix("let "))
        .unwrap_or(line);
    let name = identifier(rest);
    (!name.is_empty() && !line.starts_with(' ')).then(|| name.to_string())
}

/// Every text between `open` and the next `close`, skipping computed names.
fn quoted<'a>(line: &'a str, open: &str, close: char) -> impl Iterator<Item = &'a str> {
    let open_len = open.len();
    line.match_indices(open).filter_map(move |(index, _)| {
        let rest = &line[index + open_len..];
        let value = &rest[..rest.find(close)?];
        (!value.contains('$')).then_some(value)
    })
}

fn dialog_controls(line: &str, owner: &str, surfaces: &mut BTreeSet<String>) {
    let keys = quoted(line, "data-d=\"", '"')
        .chain(quoted(line, "[data-d=", ']'))
        .map(str::to_string)
        .chain(
            line.trim_start()
                .starts_with("on('")
                .then(|| quoted(line, "on('", '\'').next().map(str::to_string))
                .flatten(),
        );
    surfaces.extend(keys.map(|key| format!("dialog:{owner}.{key}")));
    for quick in ["qa", "qd"] {
        if line.contains(&format!("[data-{quick}]")) || line.contains(&format!("[data-{quick},")) {
            surfaces.insert(format!("dialog:{owner}.{quick}"));
        }
    }
}

fn fields(line: &str, surfaces: &mut BTreeSet<String>) {
    for tag in ["<input", "<textarea", "<select"] {
        for (index, _) in line.match_indices(tag) {
            let rest = &line[index..];
            let element = &rest[..rest.find('>').unwrap_or(rest.len())];
            let id = quoted(element, " id=\"", '"').next();
            surfaces.extend(id.map(|id| format!("field:{id}")));
        }
    }
}

/// The element a handler is attached to, from the text before `.on…`.
fn receiver(before: &str) -> String {
    if let Some(start) = before.rfind("$('") {
        let inner = &before[start + 3..];
        return inner[..inner.find('\'').unwrap_or(inner.len())]
            .trim_start_matches(['#', '.'])
            .replace(' ', "-");
    }
    let start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .map_or(0, |index| index + 1);
    before[start..].trim_end_matches('.').to_string()
}

fn listeners(line: &str, owner: &str, surfaces: &mut BTreeSet<String>) {
    for (index, _) in line.match_indices(".on") {
        let rest = &line[index + ".on".len()..];
        let event = identifier(rest);
        let target = receiver(&line[..index]);
        if !event.is_empty()
            && rest[event.len()..].starts_with(" = ")
            && !target.is_empty()
            && !target.contains("data-d")
        {
            surfaces.insert(format!("listener:{owner}.{target}.{event}"));
        }
    }
    for (index, _) in line.match_indices("addEventListener('") {
        let rest = &line[index + "addEventListener('".len()..];
        let event = identifier(rest);
        let target = match receiver(&line[..index]) {
            target if target.is_empty() => "window".to_string(),
            target => target,
        };
        surfaces.insert(format!("listener:{owner}.{target}.{event}"));
    }
}

fn attributes(line: &str, surfaces: &mut BTreeSet<String>) {
    for (index, _) in line.match_indices(" data-") {
        let name = identifier(&line[index + " data-".len()..]);
        if !name.is_empty() && !name.starts_with('$') && !PASSIVE_ATTRIBUTES.contains(&name) {
            surfaces.insert(format!("attribute:{name}"));
        }
    }
}

fn handler_keys(line: &str, owner: &str, surfaces: &mut BTreeSet<String>) {
    for open in ["e.key === '", " k === '"] {
        // A bare `k` is also a loop variable outside key handlers.
        if open == " k === '" && !line.contains("e.preventDefault()") {
            continue;
        }
        for (index, _) in line.match_indices(open) {
            let rest = &line[index + open.len()..];
            let Some(end) = rest.find('\'') else { continue };
            let key = rest[..end].to_lowercase();
            let after = &rest[end..];
            let clause = format!(
                "{} {}",
                line[..index].rsplit("if (").next().unwrap_or_default(),
                after
                    .split(')')
                    .next()
                    .filter(|modifiers| modifiers.starts_with("' &&"))
                    .unwrap_or_default()
            );
            let chord = match () {
                _ if clause.contains("metaKey") => "cmd+",
                _ if clause.contains("ctrlKey") => "ctrl+",
                _ if clause.contains("e.shiftKey &&") => "shift+",
                _ => "",
            };
            surfaces.insert(format!("handler-key:{owner}.{chord}{key}"));
            let branch = after.split("else").next().unwrap_or_default();
            if branch.contains("if (e.shiftKey)") {
                surfaces.insert(format!("handler-key:{owner}.shift+{key}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_cover_controls_outside_the_action_tables() {
        let script = r#"
function schedule() {
  dialog(`<input class="field" id="s-a">`);
  layer.onclick = (e) => { e.target.closest('[data-qa],[data-qd]'); };
  $('[data-d=ok]', layer).onclick = () => {};
}
function search() {
  $('#q-full').onclick = full;
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); if (e.shiftKey) full(); else open(); }
    else if (e.ctrlKey && e.key === 's' && scoped) { toggle(); }
  });
}
function demos() {
  on('off', () => {});
  layer.onmousedown = (e) => {};
  x.innerHTML = `<div data-rec="1"></div><button data-d="${computed}"></button>`;
}
document.addEventListener('keydown', (e) => {
  if (e.key === 'x') {}
});
const narrow = 1;
"#;
        let surfaces = source_surfaces(
            script,
            "document.addEventListener('keydown'",
            "const narrow",
        );
        for id in [
            "dialog:schedule.qa",
            "dialog:schedule.qd",
            "dialog:schedule.ok",
            "field:s-a",
            "listener:search.q-full.click",
            "listener:search.input.keydown",
            "handler-key:search.enter",
            "handler-key:search.shift+enter",
            "handler-key:search.ctrl+s",
            "dialog:demos.off",
            "listener:demos.layer.mousedown",
            "attribute:rec",
        ] {
            assert!(surfaces.contains(id), "{id}: {surfaces:?}");
        }
        assert!(!surfaces.iter().any(|id| id.contains("computed")));
        assert!(
            !surfaces.iter().any(|id| id.ends_with(".x")),
            "{surfaces:?}"
        );
    }

    fn checked_in() -> CapabilityMap {
        CapabilityMap::parse(include_str!("../capabilities.toml")).unwrap()
    }

    #[test]
    fn every_owned_action_has_a_phase_operation_and_scene() {
        let map = checked_in();
        assert!(map.capability.len() > 150, "{}", map.capability.len());
    }

    #[test]
    fn demo_controls_are_test_only() {
        let map = checked_in();
        for id in ["act:demo", "act:exitDemo", "act:fakeOk"] {
            let entry = map
                .capability
                .iter()
                .find(|entry| entry.id == id)
                .unwrap_or_else(|| panic!("{id}"));
            assert!(entry.test_only, "{id}");
        }
    }

    #[test]
    fn coverage_reports_missing_and_stale_actions() {
        let map = CapabilityMap::parse(
            r#"
            [[capability]]
            id = "act:old"
            action = "Old"
            operation = "x"
            test_only = true
            [[capability]]
            id = "gesture:drag"
            action = "Drag"
            operation = "x"
            test_only = true
            "#,
        )
        .unwrap();
        let error = map
            .check_coverage(&BTreeSet::from(["act:new".to_string()]))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("act:new") && error.contains("act:old"),
            "{error}"
        );
        assert!(!error.contains("gesture:drag"), "{error}");
    }

    #[test]
    fn owned_actions_cannot_omit_their_scenes() {
        let text = r#"
            [[capability]]
            id = "act:x"
            action = "X"
            operation = "core"
            phase = "foundation"
            acceptance = "works"
            "#;
        assert!(CapabilityMap::parse(text).is_err());
    }
}

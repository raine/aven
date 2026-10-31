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
    /// `act:`, `chord:`, `key:`, `composer:`, `palette:` or `gesture:`.
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

#[cfg(test)]
mod tests {
    use super::*;

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

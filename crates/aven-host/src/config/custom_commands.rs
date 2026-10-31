use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CUSTOM_COMMAND_TIMEOUT_SECONDS: u64 = 300;
pub(super) const MAX_CUSTOM_COMMAND_TIMEOUT_SECONDS: u64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CustomTuiCommandConfig {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub description: String,
    pub program: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_keys: Option<Vec<String>>,
    pub target: CustomTuiCommandTarget,
    #[serde(default)]
    pub execution: CustomTuiCommandExecution,
    #[serde(default)]
    pub on_success: CustomTuiCommandSuccess,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomTuiCommandConfigInput {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    description: String,
    program: PathBuf,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    detail_keys: Option<Vec<String>>,
    target: Option<CustomTuiCommandTarget>,
    requires: Option<CustomTuiCommandRequirement>,
    #[serde(default)]
    execution: CustomTuiCommandExecution,
    #[serde(default)]
    on_success: CustomTuiCommandSuccess,
}

impl<'de> Deserialize<'de> for CustomTuiCommandConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let input = CustomTuiCommandConfigInput::deserialize(deserializer)?;
        if input.target.is_some() && input.requires.is_some() {
            return Err(serde::de::Error::custom(
                "custom command cannot supply both target and requires",
            ));
        }
        let target = input.target.unwrap_or_else(|| {
            input
                .requires
                .map(CustomTuiCommandRequirement::target)
                .unwrap_or_default()
        });
        Ok(Self {
            name: input.name,
            aliases: input.aliases,
            description: input.description,
            program: input.program,
            cwd: input.cwd,
            env: input.env,
            timeout_seconds: input.timeout_seconds,
            args: input.args,
            keys: input.keys,
            detail_keys: input.detail_keys,
            target,
            execution: input.execution,
            on_success: input.on_success,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CustomTuiCommandTarget {
    None,
    #[default]
    Focused,
    Marked,
    MarkedOrFocused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CustomTuiCommandRequirement {
    None,
    SelectedTask,
}

impl CustomTuiCommandRequirement {
    fn target(self) -> CustomTuiCommandTarget {
        match self {
            Self::None => CustomTuiCommandTarget::None,
            Self::SelectedTask => CustomTuiCommandTarget::Focused,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomTuiCommandExecution {
    Background,
    #[default]
    Wait,
    Terminal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CustomTuiCommandSuccess {
    #[default]
    Stay,
    Refresh,
    Quit,
    RefreshAndQuit,
}

/// The TUI's command catalogue, which custom commands must not collide with.
pub struct TuiCommandRules {
    pub built_in_names: fn() -> Vec<&'static str>,
    pub validate_keys: fn(&[CustomTuiCommandConfig]) -> Result<()>,
}

static TUI_COMMAND_RULES: OnceLock<TuiCommandRules> = OnceLock::new();

/// Installs the rules that config validation applies. The process that hosts
/// the TUI calls this before loading config.
pub fn install_tui_command_rules(rules: TuiCommandRules) {
    let _ = TUI_COMMAND_RULES.set(rules);
}

pub(super) fn validate(commands: &[CustomTuiCommandConfig]) -> Result<()> {
    let rules = TUI_COMMAND_RULES.get();
    let mut command_names = rules
        .map(|rules| (rules.built_in_names)())
        .unwrap_or_default()
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    for command in commands {
        if command.description.trim().is_empty() {
            bail!(
                "custom command {} description must not be blank",
                command.name
            );
        }
        if command.program.as_os_str().is_empty()
            || command.program.to_string_lossy().trim().is_empty()
        {
            bail!("custom command {} program must not be blank", command.name);
        }
        if command
            .cwd
            .as_ref()
            .is_some_and(|cwd| cwd.as_os_str().is_empty())
        {
            bail!("custom command {} cwd must not be blank", command.name);
        }
        for (name, value) in &command.env {
            if name.is_empty() || name.contains('=') || name.contains('\0') {
                bail!(
                    "custom command {} has invalid environment variable name {name:?}",
                    command.name
                );
            }
            if value.contains('\0') {
                bail!(
                    "custom command {} environment variable {name:?} contains a NUL byte",
                    command.name
                );
            }
        }
        if let Some(timeout_seconds) = command.timeout_seconds
            && !(1..=MAX_CUSTOM_COMMAND_TIMEOUT_SECONDS).contains(&timeout_seconds)
        {
            bail!(
                "custom command {} timeout_seconds must be between 1 and {}",
                command.name,
                MAX_CUSTOM_COMMAND_TIMEOUT_SECONDS
            );
        }
        if command.execution == CustomTuiCommandExecution::Background
            && command.on_success != CustomTuiCommandSuccess::Stay
        {
            bail!(
                "custom command {} background execution requires on_success: stay",
                command.name
            );
        }
        for name in
            std::iter::once(command.name.as_str()).chain(command.aliases.iter().map(String::as_str))
        {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
            {
                bail!("invalid custom command name {name:?}");
            }
            if !command_names.insert(name.to_string()) {
                bail!("duplicate or built-in custom command name {name}");
            }
        }
    }
    match rules {
        Some(rules) => (rules.validate_keys)(commands),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::test_support::load_config;
    use super::*;

    #[test]
    fn custom_tui_commands_deserialize_and_validate() {
        let config = load_config(
                "tui:\n  commands:\n    - name: dispatch\n      aliases: [custom-dispatch]\n      description: Dispatch selected task\n      program: ~/bin/dispatch\n      args: [--tmux]\n      keys: [z d, D]\n      detail_keys: [z D]\n      requires: selected-task\n      execution: wait\n      on_success: quit\n",
            )
            .unwrap();
        let command = &config.tui.commands[0];

        assert_eq!(command.name, "dispatch");
        assert_eq!(command.aliases, ["custom-dispatch"]);
        assert_eq!(command.args, ["--tmux"]);
        assert_eq!(command.keys, ["z d", "D"]);
        assert_eq!(command.detail_keys.as_ref().unwrap(), &["z D".to_string()]);
        assert_eq!(command.target, CustomTuiCommandTarget::Focused);
        assert_eq!(command.execution, CustomTuiCommandExecution::Wait);
        assert_eq!(command.on_success, CustomTuiCommandSuccess::Quit);

        let terminal = load_config(
                "tui:\n  commands:\n    - name: agent\n      description: Interactive agent\n      program: agent\n      execution: terminal\n      on_success: refresh-and-quit\n",
            )
            .unwrap();
        assert_eq!(
            terminal.tui.commands[0].execution,
            CustomTuiCommandExecution::Terminal
        );
        assert_eq!(
            terminal.tui.commands[0].on_success,
            CustomTuiCommandSuccess::RefreshAndQuit
        );
    }

    #[test]
    fn custom_tui_command_static_execution_settings_deserialize_with_compatible_defaults() {
        let configured = load_config(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      cwd: ~/code/tools\n      env:\n        PROFILE: staging\n        NO_COLOR: '1'\n      timeout_seconds: 30\n",
            )
            .unwrap();
        let command = &configured.tui.commands[0];
        assert_eq!(command.cwd.as_deref(), Some(Path::new("~/code/tools")));
        assert_eq!(command.env["PROFILE"], "staging");
        assert_eq!(command.env["NO_COLOR"], "1");
        assert_eq!(command.timeout_seconds, Some(30));

        let defaulted = load_config(
                "tui:\n  commands:\n    - name: defaulted\n      description: Defaulted\n      program: dispatch\n",
            )
            .unwrap();
        let command = &defaulted.tui.commands[0];
        assert_eq!(command.cwd, None);
        assert!(command.env.is_empty());
        assert_eq!(command.timeout_seconds, None);
    }

    #[test]
    fn custom_tui_commands_validate_timeout_bounds() {
        for value in [0, MAX_CUSTOM_COMMAND_TIMEOUT_SECONDS + 1] {
            let yaml = format!(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      timeout_seconds: {value}\n"
            );
            let error = load_config(&yaml).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("timeout_seconds must be between 1 and 86400"));
        }

        let yaml = format!(
            "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      execution: background\n      timeout_seconds: {}\n",
            MAX_CUSTOM_COMMAND_TIMEOUT_SECONDS
        );
        assert!(load_config(&yaml).is_ok());
    }

    #[test]
    fn custom_tui_commands_reject_invalid_environment_without_exposing_values() {
        for (environment, expected) in [
            ("        '': value\n", "invalid environment variable name"),
            (
                "        BAD=NAME: value\n",
                "invalid environment variable name",
            ),
            (
                "        \"BAD\\u0000NAME\": value\n",
                "invalid environment variable name",
            ),
            (
                "        SAFE_NAME: \"secret-marker\\u0000suffix\"\n",
                "environment variable \"SAFE_NAME\" contains a NUL byte",
            ),
        ] {
            let yaml = format!(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      env:\n{environment}"
            );
            let error = load_config(&yaml).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("secret-marker"), "{message}");
            assert!(!message.contains("suffix"), "{message}");
        }
    }

    #[test]
    fn custom_tui_commands_reject_unknown_fields_with_the_typo_location() {
        let error = load_config(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      timeot_seconds: 30\n",
            )
            .unwrap_err();
        let message = format!("{error:#}");

        assert!(
            message.contains("unknown field `timeot_seconds`"),
            "{message}"
        );
        assert!(message.contains("timeot_seconds: 30"), "{message}");
        assert!(message.contains("line 6"), "{message}");
    }

    #[test]
    fn custom_tui_command_target_policies_and_legacy_requirements_are_compatible() {
        for (field, expected) in [
            ("target: none", CustomTuiCommandTarget::None),
            ("target: focused", CustomTuiCommandTarget::Focused),
            ("target: marked", CustomTuiCommandTarget::Marked),
            (
                "target: marked-or-focused",
                CustomTuiCommandTarget::MarkedOrFocused,
            ),
            ("requires: none", CustomTuiCommandTarget::None),
            ("requires: selected-task", CustomTuiCommandTarget::Focused),
        ] {
            let yaml = format!(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      {field}\n"
            );
            let config = load_config(&yaml).unwrap();
            assert_eq!(config.tui.commands[0].target, expected, "{field}");
        }

        let defaulted = load_config(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n",
            )
            .unwrap();
        assert_eq!(
            defaulted.tui.commands[0].target,
            CustomTuiCommandTarget::Focused
        );
    }

    #[test]
    fn custom_tui_commands_reject_target_with_legacy_requires() {
        let error = load_config(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      target: marked\n      requires: selected-task\n",
            )
            .unwrap_err();

        assert!(format!("{error:#}").contains("cannot supply both target and requires"));
    }

    #[test]
    fn custom_tui_commands_serialize_target_policy() {
        let config = load_config(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      requires: none\n",
            )
            .unwrap();
        let yaml = serde_saphyr::to_string(&config).unwrap();

        assert!(yaml.contains("target: none"));
        assert!(!yaml.contains("requires:"));
    }

    #[test]
    fn custom_tui_command_success_policies_deserialize() {
        for (value, expected) in [
            ("stay", CustomTuiCommandSuccess::Stay),
            ("refresh", CustomTuiCommandSuccess::Refresh),
            ("quit", CustomTuiCommandSuccess::Quit),
            ("refresh-and-quit", CustomTuiCommandSuccess::RefreshAndQuit),
        ] {
            let yaml = format!(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      on_success: {value}\n"
            );
            let config = load_config(&yaml).unwrap();
            assert_eq!(config.tui.commands[0].on_success, expected, "{value}");
        }
    }

    #[test]
    fn custom_tui_commands_reject_blank_fields_and_non_stay_background_policies() {
        for yaml in [
            "tui:\n  commands:\n    - name: dispatch\n      description: '  '\n      program: dispatch\n",
            "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: ''\n",
            "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      execution: background\n      on_success: refresh\n",
            "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      execution: background\n      on_success: quit\n",
            "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      execution: background\n      on_success: refresh-and-quit\n",
        ] {
            assert!(load_config(yaml).is_err(), "accepted {yaml}");
        }

        let stay = load_config(
                "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      execution: background\n      on_success: stay\n",
            )
            .unwrap();
        assert_eq!(
            stay.tui.commands[0].on_success,
            CustomTuiCommandSuccess::Stay
        );
    }
}

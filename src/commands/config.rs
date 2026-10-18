use anyhow::{Context, Result, bail};

use crate::cli::{ConfigCommand, ConfigKey, ConfigSubcommand};
use crate::config::{AppConfig, ImageOptimizationConfig, QrGlyphsConfig, config_file_path};
use crate::config_edit::{set_scalar, yaml_scalar};
use crate::operations::{init_config, show_config};
use crate::render::quote;

pub(crate) async fn cmd_config(args: ConfigCommand) -> Result<()> {
    match args.command {
        ConfigSubcommand::Init => {
            let outcome = init_config()?;
            println!(
                "created-config path={}",
                quote(&outcome.path.display().to_string())
            );
        }
        ConfigSubcommand::Show => {
            let outcome = show_config()?;
            println!("config path={}", quote(&outcome.path.display().to_string()));
            println!("{}", outcome.text);
        }
        ConfigSubcommand::Get(args) => {
            let config = AppConfig::load()?;
            println!("{}", args.key.render(&config));
        }
        ConfigSubcommand::Set(args) => {
            let value = args.key.parse_value(&args.value)?;
            let (section, key) = args.key.path();
            set_scalar(&config_file_path()?, section, key, &value)?;
            println!("updated-config key={}", args.key.name());
        }
    }
    Ok(())
}

impl ConfigKey {
    fn name(self) -> &'static str {
        match self {
            Self::SyncEnabled => "sync.enabled",
            Self::RemindersEnabled => "reminders.enabled",
            Self::SyncIntervalSeconds => "sync.interval_seconds",
            Self::SyncQrGlyphs => "sync.qr_glyphs",
            Self::UpdateAutomaticChecks => "update.automatic_checks",
            Self::LocalDbPath => "local.db_path",
            Self::LocalImageOptimization => "local.image_optimization",
        }
    }

    fn path(self) -> (&'static str, &'static str) {
        self.name()
            .split_once('.')
            .expect("config keys contain a section")
    }

    fn render(self, config: &AppConfig) -> String {
        match self {
            Self::SyncEnabled => config.sync.enabled.to_string(),
            Self::RemindersEnabled => config.reminders.enabled.to_string(),
            Self::SyncIntervalSeconds => config.sync_interval_seconds().to_string(),
            Self::SyncQrGlyphs => match config.sync.qr_glyphs {
                QrGlyphsConfig::Auto => "auto".to_string(),
                QrGlyphsConfig::Sextant => "sextant".to_string(),
                QrGlyphsConfig::HalfBlock => "half-block".to_string(),
            },
            Self::UpdateAutomaticChecks => config.update.automatic_checks.to_string(),
            Self::LocalDbPath => render_optional_string(
                config
                    .local
                    .db_path
                    .as_ref()
                    .map(|path| path.to_string_lossy())
                    .as_deref(),
            ),
            Self::LocalImageOptimization => match config.local.image_optimization {
                ImageOptimizationConfig::Off => "off".to_string(),
                ImageOptimizationConfig::Paste => "paste".to_string(),
                ImageOptimizationConfig::On => "on".to_string(),
            },
        }
    }

    fn parse_value(self, value: &str) -> Result<String> {
        match self {
            Self::SyncEnabled | Self::RemindersEnabled | Self::UpdateAutomaticChecks => match value
            {
                "true" | "false" => Ok(value.to_string()),
                _ => bail!("invalid value for {}: expected true or false", self.name()),
            },
            Self::SyncIntervalSeconds => {
                let seconds = value.parse::<u64>().with_context(|| {
                    format!(
                        "invalid value for {}: expected a positive integer",
                        self.name()
                    )
                })?;
                if seconds == 0 {
                    bail!(
                        "invalid value for {}: expected a positive integer",
                        self.name()
                    );
                }
                Ok(seconds.to_string())
            }
            Self::LocalDbPath => {
                if value == "null" {
                    return Ok("null".to_string());
                }
                if value.trim().is_empty() {
                    bail!("invalid value for {}: path must not be empty", self.name());
                }
                Ok(yaml_scalar(value))
            }
            Self::SyncQrGlyphs => match value {
                "auto" | "sextant" | "half-block" => Ok(value.to_string()),
                _ => bail!(
                    "invalid value for {}: expected auto, sextant, or half-block",
                    self.name()
                ),
            },
            Self::LocalImageOptimization => match value {
                "off" | "paste" | "on" => Ok(value.to_string()),
                _ => bail!(
                    "invalid value for {}: expected off, paste, or on",
                    self.name()
                ),
            },
        }
    }
}

fn render_optional_string(value: Option<&str>) -> String {
    value
        .map(|value| serde_json::to_string(value).expect("strings serialize to JSON"))
        .unwrap_or_else(|| "null".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_db_path_serializes_long_strings_on_one_line() {
        let path = format!("/data/{}/db.sqlite", "long path ".repeat(32));
        let value = ConfigKey::LocalDbPath.parse_value(&path).unwrap();
        assert!(!value.contains('\n'));
        let config: AppConfig =
            serde_saphyr::from_str(&format!("local:\n  db_path: {value}\n")).unwrap();
        assert_eq!(
            config.local.db_path.unwrap(),
            std::path::PathBuf::from(path)
        );
    }
}

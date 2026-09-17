use anyhow::{Result, bail};

use crate::ai;
use crate::cli::{AiCommand, AiSubcommand};
use crate::config::AppConfig;

pub(crate) async fn cmd_ai(config: &AppConfig, args: AiCommand) -> Result<()> {
    match args.command {
        AiSubcommand::Providers => {
            for provider in ai::providers() {
                println!(
                    "{} auth={} schema={}",
                    provider.id,
                    provider.auth.description(),
                    provider.schema.description()
                );
            }
            for (provider, reason) in ai::exclusions() {
                println!("{provider} excluded={reason}");
            }
            Ok(())
        }
        AiSubcommand::Login { provider } => {
            let provider = provider
                .as_deref()
                .or(config.ai.provider.as_deref())
                .ok_or_else(|| anyhow::anyhow!("error ai-provider-required hint=\"configure ai.provider or pass a provider\""))?;
            ai::login(provider).await?;
            println!("authenticated provider={provider}");
            Ok(())
        }
        AiSubcommand::Status { json } => {
            let capabilities = ai::AiService::from_config(config).capabilities();
            if json {
                println!("{}", serde_json::to_string_pretty(&capabilities)?);
            } else {
                let status = serde_json::to_value(&capabilities)?
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("temporarily_unavailable")
                    .to_string();
                println!(
                    "status={status} provider={} model={}",
                    config.ai.provider.as_deref().unwrap_or("none"),
                    config.ai.model.as_deref().unwrap_or("none")
                );
            }
            Ok(())
        }
        AiSubcommand::Check => {
            if config.ai.provider.is_none() {
                bail!("error ai-unconfigured hint=\"configure ai.provider and ai.model\"");
            }
            let response = ai::check(config).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            Ok(())
        }
    }
}

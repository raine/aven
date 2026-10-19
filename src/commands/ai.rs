use std::io::{self, IsTerminal};

use anyhow::{Result, bail};
use crossterm::style::{Color, Stylize};

use crate::ai;
use crate::cli::{AiCommand, AiSubcommand};
use crate::server_config::{ServerConfig, ServerPaths};

pub(crate) async fn cmd_ai(paths: ServerPaths, args: AiCommand) -> Result<()> {
    match args.command {
        AiSubcommand::Providers => {
            for provider in ai::providers() {
                println!(
                    "{} auth={} schema={} tools={}",
                    provider.id,
                    provider.auth.description(),
                    provider.schema.description(),
                    provider.tools
                );
            }
            for (provider, reason) in ai::exclusions() {
                println!("{provider} excluded={reason}");
            }
            Ok(())
        }
        AiSubcommand::Login { provider } => {
            let config = ServerConfig::load(paths)?;
            let provider = provider
                .as_deref()
                .or(config.ai.provider.as_deref())
                .ok_or_else(|| anyhow::anyhow!("error ai-provider-required hint=\"configure ai.provider in server.yaml or pass a provider\""))?;
            ai::login(provider, &config.paths.ai_credentials_dir()).await?;
            let styled = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
            println!("{}", format_login_success(provider, styled));
            Ok(())
        }
        AiSubcommand::Status { json } => {
            let config = ServerConfig::load(paths)?;
            let capabilities = ai::AiService::from_config(&config).capabilities();
            if json {
                println!("{}", serde_json::to_string_pretty(&capabilities)?);
            } else {
                let status = serde_json::to_value(&capabilities)?
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("temporarily_unavailable")
                    .to_string();
                let sources = config.ai_sources;
                let endpoint = config
                    .ai
                    .endpoint
                    .as_deref()
                    .map(|endpoint| {
                        format!(
                            " endpoint={endpoint} endpoint_source={}",
                            sources.endpoint.as_str()
                        )
                    })
                    .unwrap_or_default();
                println!(
                    "status={status} provider={} provider_source={} model={} model_source={}{endpoint} config={}",
                    config.ai.provider.as_deref().unwrap_or("none"),
                    sources.provider.as_str(),
                    config.ai.model.as_deref().unwrap_or("none"),
                    sources.model.as_str(),
                    config.paths.config_file.display()
                );
            }
            Ok(())
        }
        AiSubcommand::Check => {
            let config = ServerConfig::load(paths)?;
            if config.ai.provider.is_none() {
                bail!(
                    "error ai-unconfigured hint=\"configure ai.provider and ai.model in server.yaml\""
                );
            }
            let response = ai::check(&config).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            Ok(())
        }
    }
}

fn format_login_success(provider: &str, styled: bool) -> String {
    let provider = match provider {
        "chatgpt" => "ChatGPT",
        "copilot" => "GitHub Copilot",
        _ => provider,
    };
    if styled {
        format!(
            "{} {}",
            "✓".with(Color::Green).bold(),
            format!("Authenticated with {provider}")
                .with(Color::Green)
                .bold()
        )
    } else {
        format!("Authenticated with {provider}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_success_names_the_provider_and_honors_plain_output() {
        assert_eq!(
            format_login_success("chatgpt", false),
            "Authenticated with ChatGPT"
        );
        assert!(format_login_success("copilot", true).contains("\u{1b}["));
    }
}

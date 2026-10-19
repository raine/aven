use std::env;
use std::fmt;
use std::io::{self, IsTerminal};
use std::path::Path;

use anyhow::{Result, bail};
use inquire::{Confirm, InquireError, Select, Text};

use super::{check_config, format_check, styled};
use crate::ai::{self, AuthKind, ProviderInfo};
use crate::cli::AiSetupArgs;
use crate::server_config::{self, ServerConfig, ServerPaths};

const OLLAMA_ENDPOINT: &str = "http://127.0.0.1:11434";
const LLAMAFILE_ENDPOINT: &str = "http://127.0.0.1:8080";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Provider(&'static ProviderInfo),
    /// The openai integration pointed at another OpenAI-compatible server.
    Custom,
}

impl Choice {
    fn info(self) -> &'static ProviderInfo {
        match self {
            Self::Provider(info) => info,
            Self::Custom => ai::provider_info("openai").expect("openai is a registered provider"),
        }
    }
}

struct ChoiceItem {
    choice: Choice,
    label: String,
}

impl fmt::Display for ChoiceItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label)
    }
}

/// Tool-capable providers, tested ones first, then the rest of the registry
/// and a custom OpenAI-compatible endpoint.
fn provider_choices() -> Vec<ChoiceItem> {
    let tested = |id: &str| ai::TESTED.iter().any(|(tested, _)| *tested == id);
    let item = |info: &'static ProviderInfo| {
        let auth = match info.auth {
            AuthKind::OAuth => format!("log in with {}", ai::provider_display_name(info.id)),
            AuthKind::ApiKey(name) => name.to_string(),
            AuthKind::None => "local server, no key".to_string(),
        };
        let note = if tested(info.id) { "tested, " } else { "" };
        ChoiceItem {
            choice: Choice::Provider(info),
            label: format!("{:<18} {note}{auth}", info.id),
        }
    };
    let mut items = ai::TESTED
        .iter()
        .filter_map(|(id, _)| ai::provider_info(id))
        .map(item)
        .collect::<Vec<_>>();
    items.extend(
        ai::providers()
            .iter()
            .filter(|info| info.tools && !tested(info.id))
            .map(item),
    );
    items.push(ChoiceItem {
        choice: Choice::Custom,
        label: format!(
            "{:<18} another OpenAI-compatible endpoint, OPENAI_API_KEY",
            "openai-compatible"
        ),
    });
    items
}

/// The ai values server.yaml itself sets, ignoring `AVEN_AI_*` overrides.
struct FileSelection {
    provider: Option<String>,
    model: Option<String>,
    endpoint: Option<String>,
}

impl FileSelection {
    fn choice(&self) -> Option<Choice> {
        let info = ai::provider_info(self.provider.as_deref()?)?;
        Some(if info.id == "openai" && self.endpoint.is_some() {
            Choice::Custom
        } else {
            Choice::Provider(info)
        })
    }

    fn selects(&self, provider: &str) -> bool {
        self.provider.as_deref() == Some(provider)
    }
}

pub(super) async fn setup(paths: ServerPaths, args: AiSetupArgs) -> Result<()> {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if !interactive && (args.provider.is_none() || args.model.is_none()) {
        bail!(
            "aven server ai setup needs a terminal for its prompts; pass --provider and --model to run it without one"
        );
    }
    let current = ServerConfig::load_file(paths.clone())?;
    let config_file = current.paths.config_file.clone();
    let credentials_dir = current.paths.ai_credentials_dir();
    let file = FileSelection {
        provider: current.ai.provider,
        model: current.ai.model,
        endpoint: current.ai.endpoint,
    };
    let overrides = server_config::AI_OVERRIDE_ENVS
        .into_iter()
        .filter(|name| env::var_os(name).is_some_and(|value| !value.is_empty()))
        .collect::<Vec<_>>();
    if let Some(warning) = override_warning(&overrides) {
        println!("{warning}\n");
    }

    let choice = match args.provider.as_deref() {
        Some(provider) => {
            let Some(info) = ai::provider_info(provider) else {
                bail!(
                    "unknown AI provider {provider}; run `aven server ai providers` to list providers"
                );
            };
            Choice::Provider(info)
        }
        None => choose_provider(file.choice())?,
    };
    let info = choice.info();
    if !info.tools {
        bail!(
            "{} cannot carry tool calls, which the Ask Aven agent needs; choose another provider",
            info.id
        );
    }
    let provider = info.id;
    let model = match args.model {
        Some(model) => model,
        None => choose_model(provider, &file)?,
    };
    let endpoint = match args.endpoint {
        Some(endpoint) => Some(endpoint),
        None if interactive => choose_endpoint(choice, &file)?,
        None => file
            .endpoint
            .clone()
            .filter(|_| file.selects(provider) && supports_endpoint(provider)),
    };
    server_config::validate_ai_selection(provider, &model, endpoint.as_deref())?;

    println!(
        "{}",
        format_summary(&config_file, provider, &model, endpoint.as_deref(), &file)
    );
    if interactive
        && !args.yes
        && !prompt(
            Confirm::new("Write these settings?")
                .with_default(true)
                .prompt(),
        )?
    {
        println!("Nothing changed.");
        return Ok(());
    }

    let mut check_skip = None;
    match info.auth {
        AuthKind::OAuth => {
            let logged_in = ai::credentials_present(*info, Some(&credentials_dir));
            let relogin = logged_in
                && interactive
                && !args.yes
                && prompt(
                    Confirm::new(&format!(
                        "Already logged in to {}. Log in again?",
                        ai::provider_display_name(provider)
                    ))
                    .with_default(false)
                    .prompt(),
                )?;
            if !logged_in || relogin {
                ai::login(provider, &credentials_dir).await?;
                println!(
                    "Logged in. Credentials saved in {}.\n",
                    credentials_dir.join(provider).display()
                );
            }
        }
        AuthKind::ApiKey(name) => {
            println!("{}\n", api_key_note(provider, name));
            if !ai::credentials_present(*info, None) {
                check_skip = Some(format!(
                    "Skipped the check because {name} is not set in this shell. Set it here and run `aven server ai check` to verify the setup."
                ));
            }
        }
        AuthKind::None => {}
    }

    server_config::write_ai_selection(&config_file, provider, &model, endpoint.as_deref())?;
    println!("Updated {}.\n", config_file.display());

    if args.no_check {
        check_skip =
            Some("Skipped the check. Run `aven server ai check` to verify the setup.".to_string());
    }
    match check_skip {
        Some(message) => println!("{message}"),
        None => {
            let config = ServerConfig::load(paths)?;
            let report = check_config(&config, true).await;
            print!("{}", format_check(&report, styled()));
            if !report.ok {
                println!(
                    "\nserver.yaml is updated, but the check failed. Fix the problem above, run\n\
                     `aven server ai check`, then restart the server."
                );
                bail!("AI check failed");
            }
        }
    }
    println!("\n{}", restart_instructions(detect_deployment()));
    Ok(())
}

fn prompt<T>(result: std::result::Result<T, InquireError>) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            bail!("setup canceled; nothing changed")
        }
        Err(error) => Err(error.into()),
    }
}

fn choose_provider(current: Option<Choice>) -> Result<Choice> {
    let items = provider_choices();
    let start = current
        .and_then(|choice| items.iter().position(|item| item.choice == choice))
        .unwrap_or(0);
    let item = prompt(
        Select::new("AI provider:", items)
            .with_starting_cursor(start)
            .with_page_size(12)
            .with_help_message("tool-capable providers; type to filter")
            .prompt(),
    )?;
    Ok(item.choice)
}

fn choose_model(provider: &str, file: &FileSelection) -> Result<String> {
    let default = file
        .model
        .clone()
        .filter(|_| file.selects(provider))
        .or_else(|| {
            ai::TESTED
                .iter()
                .find(|(tested, _)| *tested == provider)
                .map(|(_, model)| model.to_string())
        });
    let mut text = Text::new("Model:")
        .with_help_message("the model must support tool calling")
        .with_validator(|value: &str| {
            Ok(if value.trim().is_empty() {
                inquire::validator::Validation::Invalid("enter a model name".into())
            } else {
                inquire::validator::Validation::Valid
            })
        });
    if let Some(default) = default.as_deref() {
        text = text.with_default(default);
    }
    Ok(prompt(text.prompt())?.trim().to_string())
}

fn supports_endpoint(provider: &str) -> bool {
    matches!(provider, "llamafile" | "ollama" | "openai")
}

fn choose_endpoint(choice: Choice, file: &FileSelection) -> Result<Option<String>> {
    let provider = choice.info().id;
    let default = match choice {
        Choice::Provider(info) if info.id == "ollama" => OLLAMA_ENDPOINT,
        Choice::Provider(info) if info.id == "llamafile" => LLAMAFILE_ENDPOINT,
        Choice::Custom => "",
        Choice::Provider(_) => return Ok(None),
    };
    let current = file
        .endpoint
        .as_deref()
        .filter(|_| file.selects(provider))
        .unwrap_or(default);
    let mut text = Text::new("Endpoint URL:")
        .with_help_message(
            "as reached from the server; in a container the host is usually host.docker.internal",
        )
        .with_validator(|value: &str| {
            Ok(match url::Url::parse(value.trim()) {
                Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => {
                    inquire::validator::Validation::Valid
                }
                _ => inquire::validator::Validation::Invalid(
                    "enter an absolute http or https URL".into(),
                ),
            })
        });
    if !current.is_empty() {
        text = text.with_default(current);
    }
    Ok(Some(prompt(text.prompt())?.trim().to_string()))
}

fn override_warning(overrides: &[&str]) -> Option<String> {
    let (last, rest) = overrides.split_last()?;
    let names = if rest.is_empty() {
        format!("{last} is")
    } else {
        format!("{} and {last} are", rest.join(", "))
    };
    Some(format!(
        "Warning: {names} set in this shell and override server.yaml. The check below\n\
         uses the overridden values, and a server started with the same variables\n\
         ignores what setup writes for them."
    ))
}

fn format_summary(
    config_file: &Path,
    provider: &str,
    model: &str,
    endpoint: Option<&str>,
    file: &FileSelection,
) -> String {
    let mut text = format!(
        "Settings for {}:\n  ai.provider: {provider}\n  ai.model:    {model}\n",
        config_file.display()
    );
    match (endpoint, file.endpoint.is_some()) {
        (Some(endpoint), _) => text.push_str(&format!("  ai.endpoint: {endpoint}\n")),
        (None, true) => text.push_str("  ai.endpoint: removed\n"),
        (None, false) => {}
    }
    text
}

fn api_key_note(provider: &str, name: &str) -> String {
    let extra = if provider == "azure" {
        " Azure also needs AZURE_ENDPOINT and AZURE_API_VERSION."
    } else {
        ""
    };
    format!(
        "{provider} reads {name} from the server's own environment, not from server.yaml.{extra}\n\
         Set it where the server process starts, for example an EnvironmentFile in the\n\
         systemd unit or the Compose env_file."
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deployment {
    Systemd,
    Container,
    Other,
}

fn detect_deployment() -> Deployment {
    if env::var_os("INVOCATION_ID").is_some_and(|value| !value.is_empty()) {
        Deployment::Systemd
    } else if Path::new("/.dockerenv").exists() {
        Deployment::Container
    } else {
        Deployment::Other
    }
}

fn restart_instructions(deployment: Deployment) -> String {
    let restart = match deployment {
        Deployment::Systemd => {
            "Restart the server to apply the change, using your unit's name and scope:\n\n  \
             systemctl --user restart aven-server"
        }
        Deployment::Container => {
            "Restart the container from the host to apply the change, for example:\n\n  \
             docker compose restart aven"
        }
        Deployment::Other => {
            "Restart the server to apply the change: stop `aven server` and start it\n\
             again with the same options."
        }
    };
    format!(
        "{restart}\n\nAfter the restart, the server logs an `ai status=` line that says whether AI\nis ready."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_list_tested_ones_first_and_only_tool_capable_ones() {
        let items = provider_choices();
        let ids = items
            .iter()
            .map(|item| item.label.split_whitespace().next().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids[..2], ["chatgpt", "openai"]);
        assert!(items[0].label.contains("tested, log in with ChatGPT"));
        assert!(items[1].label.contains("tested, OPENAI_API_KEY"));
        assert!(ids.contains(&"ollama"));
        assert!(!ids.contains(&"perplexity"));
        assert_eq!(ids.iter().filter(|id| **id == "openai").count(), 1);
        assert_eq!(ids.last(), Some(&"openai-compatible"));
        assert!(items.last().unwrap().choice == Choice::Custom);
    }

    #[test]
    fn override_warning_names_each_variable() {
        assert_eq!(override_warning(&[]), None);
        assert!(
            override_warning(&["AVEN_AI_MODEL"])
                .unwrap()
                .starts_with("Warning: AVEN_AI_MODEL is set")
        );
        assert!(
            override_warning(&["AVEN_AI_PROVIDER", "AVEN_AI_MODEL"])
                .unwrap()
                .starts_with("Warning: AVEN_AI_PROVIDER and AVEN_AI_MODEL are set")
        );
    }

    #[test]
    fn restart_instructions_match_the_deployment() {
        assert!(
            restart_instructions(Deployment::Systemd)
                .contains("systemctl --user restart aven-server")
        );
        assert!(
            restart_instructions(Deployment::Container).contains("docker compose restart aven")
        );
        assert!(restart_instructions(Deployment::Other).contains("stop `aven server`"));
    }
}

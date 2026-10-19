use std::io::{self, IsTerminal};

use anyhow::{Result, bail};
use crossterm::style::{Color, Stylize};
use serde::Serialize;

use crate::ai::{self, AuthKind, check};
use crate::cli::{AiCommand, AiSubcommand};

mod setup;
use crate::server_config::{ServerConfig, ServerPaths, SettingSource};

const SCOPE_NOTE: &str = "This reads server.yaml and the environment of this process, not the running server. Changes apply after the server restarts.";

pub(crate) async fn cmd_ai(paths: ServerPaths, args: AiCommand) -> Result<()> {
    match args.command {
        AiSubcommand::Setup(args) => setup::setup(paths, args).await,
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
        AiSubcommand::Login { provider } => login(paths, provider).await,
        AiSubcommand::Logout { provider } => logout(&paths, &provider),
        AiSubcommand::Status { json } => status(paths, json),
        AiSubcommand::Check { json } => run_check(paths, json).await,
    }
}

pub(super) fn styled() -> bool {
    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

async fn login(paths: ServerPaths, provider: Option<String>) -> Result<()> {
    let config = ServerConfig::load(paths)?;
    let Some(provider) = provider.or_else(|| config.ai.provider.clone()) else {
        print!("{}", login_choices());
        return Ok(());
    };
    let Some(info) = ai::provider_info(&provider) else {
        bail!("unknown AI provider {provider}; run `aven server ai providers` to list providers");
    };
    match info.auth {
        AuthKind::OAuth => {}
        AuthKind::ApiKey(name) => {
            println!(
                "{provider} uses an API key, not interactive login.\n\
                 The server reads {name} from its own environment. Set it where the server\n\
                 process starts (for example the systemd unit or the Compose env_file), restart\n\
                 the server, then run `aven server ai check`."
            );
            return Ok(());
        }
        AuthKind::None => {
            println!(
                "{provider} needs no login. Set ai.provider and ai.model (and ai.endpoint if it\n\
                 is not on the default address) in server.yaml, restart the server, then run\n\
                 `aven server ai check`."
            );
            return Ok(());
        }
    }
    let credentials_dir = config.paths.ai_credentials_dir();
    ai::login(&provider, &credentials_dir).await?;
    println!("{}", format_login_success(&provider, styled()));
    println!(
        "Credentials saved in {}.",
        credentials_dir.join(&provider).display()
    );
    println!("\nNext steps:");
    let selected = config.ai.provider.as_deref() == Some(provider.as_str());
    match (selected, config.ai.model.as_deref()) {
        (true, Some(model)) => println!(
            "  1. server.yaml already selects {provider} with model {model}; nothing to change."
        ),
        _ => println!(
            "  1. Set ai.provider: {provider} and ai.model in {}.",
            config.paths.config_file.display()
        ),
    }
    println!("  2. Restart the server.");
    println!("  3. Run `aven server ai check`.");
    Ok(())
}

fn login_choices() -> String {
    let mut text = String::from("Choose an OAuth provider to log in to:\n");
    for provider in ai::providers()
        .iter()
        .filter(|provider| provider.auth == AuthKind::OAuth)
    {
        text.push_str(&format!(
            "  aven server ai login {:<9} {}\n",
            provider.id,
            ai::provider_display_name(provider.id)
        ));
    }
    text.push_str(
        "\nOther providers need no login. API-key providers read their key from the\n\
         server's environment; set it there and restart the server. Run\n\
         `aven server ai providers` to see each provider's key variable.\n",
    );
    text
}

fn logout(paths: &ServerPaths, provider: &str) -> Result<()> {
    let Some(info) = ai::provider_info(provider) else {
        bail!("unknown AI provider {provider}; run `aven server ai providers` to list providers");
    };
    if let AuthKind::ApiKey(name) = info.auth {
        println!(
            "{provider} has no stored login. The server reads {name} from its own\n\
             environment; remove it there, then restart the server."
        );
        return Ok(());
    }
    if info.auth == AuthKind::None {
        println!("{provider} has no stored login.");
        return Ok(());
    }
    let name = ai::provider_display_name(provider);
    match ai::logout(provider, &paths.ai_credentials_dir())? {
        Some(path) => {
            println!("Removed stored {name} credentials from {}.", path.display());
            println!("Restart the server so it stops using them.");
            println!(
                "This does not revoke the authorization at {name}; revoke it in your account settings there if needed."
            );
        }
        None => println!(
            "No stored {name} credentials in {}.",
            paths.ai_credentials_dir().display()
        ),
    }
    Ok(())
}

#[derive(Serialize)]
struct StatusReport {
    #[serde(flatten)]
    capabilities: ai::CapabilitiesResponse,
    provider_source: SettingSource,
    model_source: SettingSource,
    endpoint: Option<String>,
    endpoint_source: SettingSource,
    config_path: String,
    config_exists: bool,
    credentials_dir: String,
}

fn status(paths: ServerPaths, json: bool) -> Result<()> {
    let config = ServerConfig::load(paths)?;
    let capabilities = ai::AiService::from_config(&config).capabilities();
    let sources = config.ai_sources;
    let report = StatusReport {
        provider_source: sources.provider,
        model_source: sources.model,
        endpoint: config.ai.endpoint.clone(),
        endpoint_source: sources.endpoint,
        config_path: config.paths.config_file.display().to_string(),
        config_exists: config.paths.config_file.is_file(),
        credentials_dir: config.paths.ai_credentials_dir().display().to_string(),
        capabilities,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_status(&report));
    }
    Ok(())
}

fn source_label(source: SettingSource, env: &str) -> String {
    match source {
        SettingSource::Unset => "not set".to_string(),
        SettingSource::File => "server.yaml".to_string(),
        SettingSource::Env => env.to_string(),
    }
}

fn setting(value: Option<&str>, source: SettingSource, env: &str) -> String {
    match value {
        Some(value) => format!("{value} ({})", source_label(source, env)),
        None => "not set".to_string(),
    }
}

fn format_status(report: &StatusReport) -> String {
    let config = if report.config_exists {
        report.config_path.clone()
    } else {
        format!("{} (not found; defaults apply)", report.config_path)
    };
    let mut text = format!(
        "Config:       {config}\n\
         Credentials:  {}\n\
         Provider:     {}\n\
         Model:        {}\n",
        report.credentials_dir,
        setting(
            report.capabilities.provider.as_deref(),
            report.provider_source,
            "AVEN_AI_PROVIDER"
        ),
        setting(
            report.capabilities.model.as_deref(),
            report.model_source,
            "AVEN_AI_MODEL"
        ),
    );
    if report.endpoint.is_some() {
        text.push_str(&format!(
            "Endpoint:     {}\n",
            setting(
                report.endpoint.as_deref(),
                report.endpoint_source,
                "AVEN_AI_ENDPOINT"
            )
        ));
    }
    text.push_str(&format!(
        "Status:       {}\n",
        report.capabilities.status.as_str()
    ));
    match &report.capabilities.setup_hint {
        Some(hint) => text.push_str(&format!("Hint:         {hint}\n")),
        None => text.push_str(
            "Hint:         run `aven server ai check` to verify the model with a billable request\n",
        ),
    }
    text.push_str(&format!("\n{SCOPE_NOTE}\n"));
    text
}

#[derive(Serialize)]
struct CheckReport {
    ok: bool,
    provider: Option<String>,
    model: Option<String>,
    config_path: String,
    steps: Vec<StepReport>,
    usage: check::Usage,
}

#[derive(Serialize)]
pub(super) struct StepReport {
    pub(super) step: &'static str,
    pub(super) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

impl StepReport {
    pub(super) fn passed(step: &'static str) -> Self {
        Self {
            step,
            ok: true,
            error: None,
            hint: None,
        }
    }

    pub(super) fn failed(step: &'static str, error: &'static str, hint: String) -> Self {
        Self {
            step,
            ok: false,
            error: Some(error),
            hint: Some(hint),
        }
    }
}

async fn run_check(paths: ServerPaths, json: bool) -> Result<()> {
    let config = ServerConfig::load(paths)?;
    let report = check_config(&config, !json).await;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_check(&report, styled()));
        println!("\n{SCOPE_NOTE}");
    }
    if !report.ok {
        bail!("AI check failed");
    }
    Ok(())
}

/// Runs the tool-call check with `config` from this process.
async fn check_config(config: &ServerConfig, announce: bool) -> CheckReport {
    let service = ai::AiService::from_config(config);
    let capabilities = service.capabilities();
    let mut report = CheckReport {
        ok: false,
        provider: config.ai.provider.clone(),
        model: config.ai.model.clone(),
        config_path: config.paths.config_file.display().to_string(),
        steps: Vec::new(),
        usage: check::Usage::default(),
    };
    if let Some(hint) = capabilities.setup_hint {
        report.steps.push(StepReport::failed(
            "configuration",
            capabilities.status.as_str(),
            hint,
        ));
    } else {
        report.steps.push(StepReport::passed("configuration"));
        if announce {
            println!(
                "Checking {} model {} from this process (two billable requests)...",
                report.provider.as_deref().unwrap_or("none"),
                report.model.as_deref().unwrap_or("none"),
            );
        }
        let outcome = ai::check(&service).await;
        report.usage = outcome.usage;
        report.steps.extend(
            outcome
                .passed
                .iter()
                .map(|step| StepReport::passed(step.as_str())),
        );
        if let Some((step, failure)) = outcome.failure {
            let (error, hint) = match failure {
                check::Failure::Completion(category) => {
                    (category.as_str(), check::category_hint(category))
                }
                check::Failure::Mismatch(mismatch) => (mismatch.as_str(), mismatch.hint()),
            };
            report
                .steps
                .push(StepReport::failed(step.as_str(), error, hint.to_string()));
        }
    }
    report.ok = report.steps.iter().all(|step| step.ok);
    report
}

fn step_description(step: &str) -> &'static str {
    match step {
        "configuration" => "provider, model and credentials",
        "server" => "running server reports AI ready",
        "tool_call" => check::Step::ToolCall.description(),
        "tool_result" => check::Step::ToolResult.description(),
        _ => "",
    }
}

fn format_check(report: &CheckReport, styled: bool) -> String {
    let mut text = format_steps(&report.steps, &report.usage, styled);
    text.push_str(&format!("\n{SCOPE_NOTE}\n"));
    text
}

/// Per-step check lines, with token usage when every step passed.
pub(super) fn format_steps(steps: &[StepReport], usage: &check::Usage, styled: bool) -> String {
    let mut text = String::new();
    for step in steps {
        let mark = match (step.ok, styled) {
            (true, true) => "✓".with(Color::Green).bold().to_string(),
            (false, true) => "✗".with(Color::Red).bold().to_string(),
            (true, false) => "✓".to_string(),
            (false, false) => "✗".to_string(),
        };
        text.push_str(&format!(
            "{mark} {:<14} {}",
            step.step,
            step_description(step.step)
        ));
        if let Some(error) = step.error {
            text.push_str(&format!(": {error}"));
        }
        text.push('\n');
        if let Some(hint) = &step.hint {
            text.push_str(&format!("  Hint: {hint}\n"));
        }
    }
    if steps.iter().all(|step| step.ok) {
        text.push_str(&format!(
            "\nTool calling works ({} input, {} output tokens).\n",
            usage.input_tokens, usage.output_tokens
        ));
    }
    text
}

fn format_login_success(provider: &str, styled: bool) -> String {
    let provider = ai::provider_display_name(provider);
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

    #[test]
    fn login_choices_list_only_oauth_providers() {
        let text = login_choices();
        assert!(text.contains("aven server ai login chatgpt"), "{text}");
        assert!(text.contains("aven server ai login copilot"), "{text}");
        assert!(!text.contains("login openai"), "{text}");
        assert!(text.contains("restart the server"), "{text}");
    }

    #[test]
    fn check_output_shows_each_step_and_the_failure_hint() {
        let report = CheckReport {
            ok: false,
            provider: Some("ollama".into()),
            model: Some("m".into()),
            config_path: "server.yaml".into(),
            steps: vec![
                StepReport::passed("configuration"),
                StepReport::failed(
                    "tool_call",
                    "no_tool_call",
                    check::Mismatch::NoToolCall.hint().into(),
                ),
            ],
            usage: check::Usage::default(),
        };
        let text = format_check(&report, false);
        assert!(text.contains("✓ configuration"), "{text}");
        assert!(
            text.contains("✗ tool_call      model calls a tool: no_tool_call"),
            "{text}"
        );
        assert!(
            text.contains("  Hint: the model answered without"),
            "{text}"
        );
    }
}

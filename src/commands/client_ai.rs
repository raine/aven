//! AI diagnostics that ask the running sync server from this device, over the
//! same network path and enrolled credential that paired devices use.
use std::time::Duration;

use anyhow::Result;
use aven_core::db::Database;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::{AiClientCommand, AiClientSubcommand};
use crate::config::AppConfig;
use crate::render::print_json_pretty;

const COMPLETION_CONTRACT: u32 = 1;
const RESPONSE_LIMIT: usize = 64 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const DOCTOR_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_NOTE: &str = "Server AI changes apply only after the server restarts.";

pub(crate) async fn cmd_ai_client(
    database: &Database,
    config: &AppConfig,
    args: AiClientCommand,
) -> Result<()> {
    match args.command {
        AiClientSubcommand::Status { json } => {
            let report = probe(database, config, true, STATUS_TIMEOUT).await?;
            if json {
                print_json_pretty(&report)
            } else {
                print_report(&report);
                Ok(())
            }
        }
    }
}

/// What the running server reported about AI to this device.
#[derive(Debug, Serialize)]
pub(super) struct AiStatusReport {
    pub(super) state: AiState,
    pub(super) server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) http_status: Option<u16>,
    pub(super) provider: Option<String>,
    pub(super) model: Option<String>,
    pub(super) setup_hint: Option<String>,
    pub(super) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) capabilities: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AiState {
    Ready,
    ToolsUnsupported,
    Unconfigured,
    AuthenticationRequired,
    TemporarilyUnavailable,
    IncompatibleContract,
    Unauthorized,
    RouteMissing,
    Unreachable,
    UnexpectedResponse,
    SyncNotSetUp,
}

impl AiState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::ToolsUnsupported => "tools_unsupported",
            Self::Unconfigured => "unconfigured",
            Self::AuthenticationRequired => "authentication_required",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::IncompatibleContract => "incompatible_contract",
            Self::Unauthorized => "unauthorized",
            Self::RouteMissing => "route_missing",
            Self::Unreachable => "unreachable",
            Self::UnexpectedResponse => "unexpected_response",
            Self::SyncNotSetUp => "sync_not_set_up",
        }
    }
}

#[derive(Deserialize)]
struct Capabilities {
    status: String,
    completion_contract: u32,
    features: Features,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    setup_hint: Option<String>,
}

#[derive(Deserialize)]
struct Features {
    tools: bool,
}

/// Asks the running server for its AI capabilities with this device's
/// credential. Fails only when the local credential can't be read.
pub(super) async fn probe(
    database: &Database,
    config: &AppConfig,
    attended: bool,
    timeout: Duration,
) -> Result<AiStatusReport> {
    let Some(credential) =
        crate::sync::encrypted::device_credential(database, config, attended).await?
    else {
        return Ok(report(
            AiState::SyncNotSetUp,
            None,
            "This database doesn't sync, so it has no server to ask. Run `aven sync setup` or \
             `aven sync join` first."
                .into(),
        ));
    };
    let server = credential.server;
    let response = match fetch(&server, credential.bearer.expose(), timeout).await {
        Ok(response) => response,
        Err(detail) => {
            return Ok(report(
                AiState::Unreachable,
                Some(server),
                format!(
                    "Couldn't reach the server: {detail}. Check the network and that the server \
                     is running."
                ),
            ));
        }
    };
    Ok(classify(server, response.0, &response.1))
}

async fn fetch(
    server: &str,
    bearer: &[u8; 32],
    timeout: Duration,
) -> std::result::Result<(u16, Vec<u8>), String> {
    let url = reqwest::Url::parse(server)
        .and_then(|url| url.join("/ai/capabilities"))
        .map_err(|_| "the sync server URL is invalid".to_string())?;
    let http = crate::sync_http::HttpDriver::new()
        .map_err(|_| "the HTTP client is unavailable".to_string())?
        .http;
    let mut authorization =
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", hex::encode(bearer)))
            .map_err(|_| "the device credential is invalid".to_string())?;
    authorization.set_sensitive(true);
    let mut response = http
        .get(url)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| describe_network_error(&error))?;
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| describe_network_error(&error))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > RESPONSE_LIMIT {
            break;
        }
    }
    Ok((status, body))
}

fn describe_network_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "the request timed out".into()
    } else if error.is_connect() {
        "the connection failed".into()
    } else {
        "the request failed".into()
    }
}

fn report(state: AiState, server: Option<String>, message: String) -> AiStatusReport {
    AiStatusReport {
        state,
        server,
        http_status: None,
        provider: None,
        model: None,
        setup_hint: None,
        message,
        capabilities: None,
    }
}

fn classify(server: String, status: u16, body: &[u8]) -> AiStatusReport {
    let mut result = match status {
        200 => return classify_capabilities(server, body),
        401 => report(
            AiState::Unauthorized,
            Some(server),
            "The server didn't accept this device's credential. Check `aven sync status`; \
             this device may have been removed from sync."
                .into(),
        ),
        404 => report(
            AiState::RouteMissing,
            Some(server),
            "The server has no AI routes. Update the server to a version with AI support, or \
             check that a reverse proxy in front of it forwards /ai/ paths."
                .into(),
        ),
        _ => report(
            AiState::UnexpectedResponse,
            Some(server),
            format!(
                "The server answered HTTP {status}. Check the server logs and any reverse proxy \
                 in front of it."
            ),
        ),
    };
    result.http_status = Some(status);
    result
}

fn classify_capabilities(server: String, body: &[u8]) -> AiStatusReport {
    let (Ok(raw), Ok(capabilities)) = (
        serde_json::from_slice::<Value>(body),
        serde_json::from_slice::<Capabilities>(body),
    ) else {
        let mut result = report(
            AiState::UnexpectedResponse,
            Some(server),
            "The server's capabilities response isn't valid. Check that a reverse proxy isn't \
             rewriting /ai/ responses."
                .into(),
        );
        result.http_status = Some(200);
        return result;
    };
    let restart = |text: &str| format!("{text} {RESTART_NOTE}");
    let (state, message) = if capabilities.completion_contract != COMPLETION_CONTRACT {
        (
            AiState::IncompatibleContract,
            format!(
                "The server speaks completion contract {}, and this aven speaks {COMPLETION_CONTRACT}. \
                 Update whichever is older.",
                capabilities.completion_contract
            ),
        )
    } else {
        match capabilities.status.as_str() {
            "ready" if capabilities.features.tools => (
                AiState::Ready,
                "The running server is ready to answer AI requests.".into(),
            ),
            "ready" => (
                AiState::ToolsUnsupported,
                restart(
                    "The configured provider can't make tool calls, which Ask Aven needs. On the \
                     server host, pick a provider with tools=true from `aven server ai providers`.",
                ),
            ),
            "unconfigured" => (
                AiState::Unconfigured,
                restart("The running server has no usable AI provider and model."),
            ),
            "authentication_required" => (
                AiState::AuthenticationRequired,
                restart("The running server has no credentials for its AI provider."),
            ),
            "temporarily_unavailable" => (
                AiState::TemporarilyUnavailable,
                restart("The running server can't use its AI provider right now."),
            ),
            other => (
                AiState::UnexpectedResponse,
                format!("The server reported an unknown AI status \"{other}\". Update aven."),
            ),
        }
    };
    AiStatusReport {
        state,
        server: Some(server),
        http_status: Some(200),
        provider: capabilities.provider,
        model: capabilities.model,
        setup_hint: capabilities.setup_hint,
        message,
        capabilities: Some(raw),
    }
}

fn print_report(report: &AiStatusReport) {
    if let Some(server) = &report.server {
        println!("Server: {server}");
    }
    println!("Status: {}", report.state.as_str());
    if let Some(status) = report.http_status.filter(|status| *status != 200) {
        println!("HTTP: {status}");
    }
    if let Some(provider) = &report.provider {
        println!("Provider: {provider}");
    }
    if let Some(model) = &report.model {
        println!("Model: {model}");
    }
    if let Some(hint) = &report.setup_hint {
        println!("Server hint: {hint}");
    }
    println!("{}", report.message);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(status: &str, tools: bool) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "status": status,
            "completion_contract": 1,
            "features": {"text": true, "json_schema": true, "tools": tools, "streaming": false, "images": false},
            "limits": {"max_request_bytes": 524288, "max_output_tokens": 4096},
            "provider": "chatgpt",
            "model": "gpt-5",
            "setup_hint": if status == "ready" { Value::Null } else { "run aven server ai login chatgpt".into() },
        }))
        .unwrap()
    }

    #[test]
    fn reports_each_server_answer_distinctly() {
        let server = || "https://sync.example".to_string();
        let ready = classify(server(), 200, &capabilities("ready", true));
        assert_eq!(ready.state, AiState::Ready);
        assert_eq!(ready.provider.as_deref(), Some("chatgpt"));
        assert_eq!(ready.model.as_deref(), Some("gpt-5"));
        assert_eq!(
            classify(server(), 200, &capabilities("ready", false)).state,
            AiState::ToolsUnsupported
        );
        let login = classify(
            server(),
            200,
            &capabilities("authentication_required", true),
        );
        assert_eq!(login.state, AiState::AuthenticationRequired);
        assert_eq!(
            login.setup_hint.as_deref(),
            Some("run aven server ai login chatgpt")
        );
        assert!(login.message.contains("restarts"));
        assert_eq!(classify(server(), 401, b"").state, AiState::Unauthorized);
        let missing = classify(server(), 404, b"not found");
        assert_eq!(missing.state, AiState::RouteMissing);
        assert_eq!(missing.http_status, Some(404));
        assert_eq!(
            classify(server(), 502, b"").state,
            AiState::UnexpectedResponse
        );
        assert_eq!(
            classify(server(), 200, b"<html>").state,
            AiState::UnexpectedResponse
        );
    }

    #[test]
    fn older_servers_without_provider_and_newer_contracts_are_explained() {
        let older = br#"{"status":"unconfigured","completion_contract":1,"features":{"tools":false},"setup_hint":"configure ai.provider and ai.model in server.yaml"}"#;
        let report = classify("https://sync.example".into(), 200, older);
        assert_eq!(report.state, AiState::Unconfigured);
        assert_eq!(report.provider, None);
        let newer = br#"{"status":"ready","completion_contract":2,"features":{"tools":true}}"#;
        assert_eq!(
            classify("https://sync.example".into(), 200, newer).state,
            AiState::IncompatibleContract
        );
    }
}

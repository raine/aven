//! `aven ai check`: the synthetic tool-call exchange sent through the running
//! server's completion route with this device's credential.
use std::time::Duration;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;

use super::{AiState, Connection, SYNC_NOT_SET_UP, http_failure, probe_with, unreachable_message};
use crate::ai::{ErrorCategory, check};
use crate::commands::ai::{StepReport, format_steps, styled};

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
// Above the server's AI timeout so the server, not this client, reports a
// slow provider.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(300);
const COMPLETION_RESPONSE_LIMIT: usize = 1024 * 1024;

#[derive(Serialize)]
struct CheckReport {
    ok: bool,
    server: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    steps: Vec<StepReport>,
    usage: check::Usage,
}

/// Why one completion through the running server did not return a response.
#[derive(Debug)]
enum RemoteError {
    /// The server's error envelope.
    Category(ErrorCategory),
    /// A failure status without an error envelope, such as 401 or 404.
    Http(u16),
    Network(String),
    Malformed,
}

impl RemoteError {
    fn describe(&self) -> (&'static str, String) {
        match self {
            Self::Category(category) => (
                category.as_str(),
                check::category_hint(*category).to_string(),
            ),
            Self::Http(status) => {
                let (state, message) = http_failure(*status);
                (state.as_str(), message)
            }
            Self::Network(detail) => (AiState::Unreachable.as_str(), unreachable_message(detail)),
            Self::Malformed => (
                check::Mismatch::MalformedResponse.as_str(),
                check::Mismatch::MalformedResponse.hint().to_string(),
            ),
        }
    }
}

pub(super) async fn run(connection: Option<Connection>, json: bool) -> Result<()> {
    let mut report = CheckReport {
        ok: false,
        server: connection
            .as_ref()
            .map(|connection| connection.server.clone()),
        provider: None,
        model: None,
        steps: Vec::new(),
        usage: check::Usage::default(),
    };
    match &connection {
        None => report.steps.push(StepReport::failed(
            "server",
            AiState::SyncNotSetUp.as_str(),
            SYNC_NOT_SET_UP.into(),
        )),
        Some(connection) => check_server(connection, &mut report, json).await,
    }
    report.ok = report.steps.iter().all(|step| step.ok);
    if json {
        crate::render::print_json_pretty(&report)?;
    } else {
        print!("{}", format_steps(&report.steps, &report.usage, styled()));
        if let Some(server) = &report.server {
            // Only the server step runs when it fails; no completion is sent.
            println!("\n{}", scope_note(server, report.steps.len() > 1));
        }
    }
    if !report.ok {
        bail!("AI check through the running server failed");
    }
    Ok(())
}

async fn check_server(connection: &Connection, report: &mut CheckReport, json: bool) {
    let status = probe_with(connection, PROBE_TIMEOUT).await;
    report.provider = status.provider.clone();
    report.model = status.model.clone();
    if status.state != AiState::Ready {
        let hint = status.setup_hint.unwrap_or(status.message);
        report
            .steps
            .push(StepReport::failed("server", status.state.as_str(), hint));
        return;
    }
    report.steps.push(StepReport::passed("server"));
    if !json {
        println!(
            "Checking {} model {} through the running server at {} from this device (two billable requests)...",
            report.provider.as_deref().unwrap_or("unknown"),
            report.model.as_deref().unwrap_or("unknown"),
            connection.server,
        );
    }
    let outcome = check::run(|request| complete(connection, request)).await;
    report.usage = outcome.usage;
    report.steps.extend(
        outcome
            .passed
            .iter()
            .map(|step| StepReport::passed(step.as_str())),
    );
    if let Some((step, failure)) = outcome.failure {
        let (error, hint) = match failure {
            check::Failure::Completion(error) => error.describe(),
            check::Failure::Mismatch(mismatch) => (mismatch.as_str(), mismatch.hint().to_string()),
        };
        report
            .steps
            .push(StepReport::failed(step.as_str(), error, hint));
    }
}

/// One completion through the running server. Response bodies stay in
/// memory for verification and are never printed.
async fn complete(connection: &Connection, request: Value) -> Result<Value, RemoteError> {
    let (status, body) = connection
        .send(
            "/ai/completions/stream",
            Some(&request),
            COMPLETION_TIMEOUT,
            COMPLETION_RESPONSE_LIMIT,
        )
        .await
        .map_err(RemoteError::Network)?;
    classify_completion(
        status,
        &body,
        request["request_id"]
            .as_str()
            .ok_or(RemoteError::Malformed)?,
    )
}

fn classify_completion(status: u16, body: &[u8], request_id: &str) -> Result<Value, RemoteError> {
    if body.len() > COMPLETION_RESPONSE_LIMIT {
        return Err(RemoteError::Malformed);
    }
    let parsed = serde_json::from_slice::<Value>(body).ok();
    if status == 200 {
        return collect_events(body, request_id);
    }
    let category = parsed
        .as_ref()
        .and_then(|body| body.pointer("/error/category"))
        .and_then(|category| serde_json::from_value(category.clone()).ok());
    Err(match category {
        Some(category) => RemoteError::Category(category),
        None => RemoteError::Http(status),
    })
}

fn collect_events(body: &[u8], request_id: &str) -> Result<Value, RemoteError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| RemoteError::Malformed)?
        .replace("\r\n", "\n");
    let mut completion = None;
    let mut event = "";
    let mut data = Vec::new();
    for line in text.lines() {
        if line.starts_with(':') {
            continue;
        }
        if line.is_empty() {
            if data.is_empty() {
                event = "";
                continue;
            }
            if completion.is_some() {
                return Err(RemoteError::Malformed);
            }
            let value: Value =
                serde_json::from_str(&data.join("\n")).map_err(|_| RemoteError::Malformed)?;
            if value["request_id"].as_str() != Some(request_id) {
                return Err(RemoteError::Malformed);
            }
            match event {
                "text_delta" if value["text"].is_string() => {}
                "completion" => completion = Some(value),
                "error" => {
                    return Err(serde_json::from_value(value["error"]["category"].clone())
                        .map(RemoteError::Category)
                        .unwrap_or(RemoteError::Malformed));
                }
                _ => return Err(RemoteError::Malformed),
            }
            data.clear();
            event = "";
        } else if let Some(value) = line.strip_prefix("event:") {
            event = value.trim();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        } else {
            return Err(RemoteError::Malformed);
        }
    }
    if !data.is_empty() || !event.is_empty() {
        return Err(RemoteError::Malformed);
    }
    completion.ok_or(RemoteError::Malformed)
}

fn scope_note(server: &str, billed: bool) -> String {
    let sent = if billed {
        "This sent billable requests through"
    } else {
        "No billable requests were sent. This asked"
    };
    format!(
        "{sent} the running server at {server} from this device, with its sync credential. \
         `aven server ai check` runs on the server host and tests server.yaml and that shell's \
         environment instead. Server AI changes apply after the server restarts."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_failures_keep_server_categories_and_route_problems_apart() {
        let envelope = br#"{"error":{"category":"unsupported_capability","retryable":false}}"#;
        let Err(RemoteError::Category(category)) = classify_completion(400, envelope, "test")
        else {
            panic!("expected a category");
        };
        assert_eq!(category, ErrorCategory::UnsupportedCapability);
        assert_eq!(
            RemoteError::Category(category).describe().1,
            check::category_hint(category)
        );
        for (status, error) in [
            (401, "unauthorized"),
            (404, "route_missing"),
            (502, "unexpected_response"),
        ] {
            let failure =
                classify_completion(status, b"<html>bad gateway</html>", "test").unwrap_err();
            assert_eq!(failure.describe().0, error);
        }
        assert_eq!(
            RemoteError::Network("the connection failed".into())
                .describe()
                .0,
            "unreachable"
        );
        assert!(matches!(
            classify_completion(200, b"not json", "test"),
            Err(RemoteError::Malformed)
        ));
        assert!(classify_completion(200, br#"{"message":{}}"#, "test").is_err());
        let frame = b"event: completion\ndata: {\"request_id\":\"test\",\"message\":{}}\n\n";
        assert!(classify_completion(200, frame, "test").is_ok());
        assert!(classify_completion(200, frame, "other").is_err());
        assert!(classify_completion(200, &frame[..frame.len() - 1], "test").is_err());
        let twice = [frame.as_slice(), frame.as_slice()].concat();
        assert!(classify_completion(200, &twice, "test").is_err());
        assert!(matches!(classify_completion(200, b"event: error\ndata: {\"request_id\":\"test\",\"error\":{\"category\":\"timeout\"}}\n\n", "test"), Err(RemoteError::Category(ErrorCategory::Timeout))));
    }

    #[test]
    fn scope_note_names_the_server_and_the_server_side_check() {
        let note = scope_note("https://sync.example", true);
        assert!(note.contains("https://sync.example"));
        assert!(note.contains("sent billable requests"));
        assert!(note.contains("aven server ai check"));
        assert!(scope_note("https://sync.example", false).starts_with("No billable requests"));
    }
}

//! Record-then-transcribe through ChatGPT's internal transcription endpoint,
//! authenticated with the server's ChatGPT login.

use std::path::PathBuf;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use bytes::Bytes;
use futures::StreamExt;
use rig_core::http_client::{self, HttpClientExt};
use rig_core::providers::chatgpt;

use super::transport::BoundedHttpClient;
use super::*;

const AUDIO_FORMAT: &str = "audio/mp4";
const MAX_AUDIO_BYTES: usize = 4 * 1024 * 1024;
const MAX_DURATION_SECONDS: u64 = 300;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const UPSTREAM_URL: &str = "https://chatgpt.com/backend-api/transcribe";

pub(crate) fn capability(status: AvailabilityStatus) -> TranscriptionCapability {
    TranscriptionCapability {
        contract: 1,
        status,
        audio_format: AUDIO_FORMAT.into(),
        max_audio_bytes: MAX_AUDIO_BYTES,
        max_duration_seconds: MAX_DURATION_SECONDS,
    }
}

/// Shares the completion backend's ChatGPT client, so rig remains the only
/// owner that refreshes and writes the stored login.
pub(crate) struct Transcriber {
    client: chatgpt::Client<BoundedHttpClient>,
    http: BoundedHttpClient,
    auth_file: PathBuf,
    url: String,
}

#[derive(Deserialize)]
struct StoredLogin {
    access_token: Option<String>,
    account_id: Option<String>,
}

/// Deserializing drops every upstream field except the text.
#[derive(Serialize, Deserialize)]
struct Transcript {
    text: String,
}

impl Transcriber {
    pub(crate) fn new(client: chatgpt::Client<BoundedHttpClient>, auth_file: PathBuf) -> Self {
        Self {
            client,
            http: BoundedHttpClient::default(),
            auth_file,
            url: UPSTREAM_URL.to_string(),
        }
    }

    /// Returns the bearer token and account id from the stored login.
    async fn login(&self) -> Result<(String, Option<String>), ErrorCategory> {
        // rig rewrites the login file in place while refreshing, so a read can
        // observe a partial write; authorizing again waits for it to finish.
        for _ in 0..2 {
            let client = self.client.clone();
            // A spawned task survives request cancellation, so a refresh the
            // auth server accepted is always persisted.
            let authorized = tokio::spawn(async move { client.authorize().await.is_ok() })
                .await
                .unwrap_or(false);
            if !authorized {
                return Err(ErrorCategory::TemporaryProviderFailure);
            }
            if let Some((token, account)) = fs::read(&self.auth_file)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<StoredLogin>(&bytes).ok())
                .and_then(|login| Some((login.access_token?, login.account_id)))
            {
                return Ok((token, account));
            }
        }
        Err(ErrorCategory::TemporaryProviderFailure)
    }

    async fn transcribe(&self, audio: Bytes) -> Result<Transcript, ErrorCategory> {
        let (token, account) = self.login().await?;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| ErrorCategory::TemporaryProviderFailure)?;
        let boundary = format!("aven-{}", hex::encode(nonce));
        // The upstream picks its decoder from the file name.
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.m4a\"\r\nContent-Type: {AUDIO_FORMAT}\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&audio);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut request = http::Request::post(&self.url)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("originator", "aven")
            .header(
                header::USER_AGENT,
                concat!("aven/", env!("CARGO_PKG_VERSION")),
            );
        if let Some(account) = account {
            request = request.header("ChatGPT-Account-Id", account);
        }
        let request = request
            .body(Bytes::from(body))
            .map_err(|_| ErrorCategory::TemporaryProviderFailure)?;
        let response = self
            .http
            .send::<_, Bytes>(request)
            .await
            .map_err(|error| match error {
                http_client::Error::InvalidStatusCodeWithDetails { status, .. } => {
                    match status.as_u16() {
                        400 => ErrorCategory::InvalidRequest,
                        401 | 403 => ErrorCategory::AuthenticationFailure,
                        429 => ErrorCategory::RateLimit,
                        _ => ErrorCategory::TemporaryProviderFailure,
                    }
                }
                _ => ErrorCategory::TemporaryProviderFailure,
            })?;
        let bytes = response
            .into_body()
            .await
            .map_err(|_| ErrorCategory::TemporaryProviderFailure)?;
        serde_json::from_slice(&bytes).map_err(|_| ErrorCategory::MalformedProviderResponse)
    }
}

impl AiService {
    pub(crate) fn transcription_status(&self) -> AvailabilityStatus {
        match &self.state {
            _ if self.provider.as_deref() != Some("chatgpt") => AvailabilityStatus::Unconfigured,
            BackendState::Ready {
                transcriber: Some(_),
                ..
            } => AvailabilityStatus::Ready,
            BackendState::Ready { .. } => AvailabilityStatus::Unconfigured,
            BackendState::Unavailable { status, .. } => *status,
        }
    }

    async fn transcribe(&self, audio: Bytes) -> Result<Transcript, ErrorCategory> {
        if self.provider.as_deref() != Some("chatgpt") {
            return Err(ErrorCategory::Unconfigured);
        }
        let BackendState::Ready {
            transcriber: Some(transcriber),
            ..
        } = &self.state
        else {
            return Err(self.unavailable_error());
        };
        self.admit()?;
        let _permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| ErrorCategory::RateLimit)?;
        tokio::time::timeout(self.timeout, transcriber.transcribe(audio))
            .await
            .map_err(|_| ErrorCategory::Timeout)?
    }
}

/// Reads the whole upload before any AI admission, so slow or invalid uploads
/// never hold a rate or concurrency slot. Rejections carry the status that
/// overrides the category's own.
async fn read_audio(headers: &HeaderMap, body: Body) -> Result<Bytes, (StatusCode, ErrorCategory)> {
    let canceled = (StatusCode::REQUEST_TIMEOUT, ErrorCategory::CanceledRequest);
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let identity = headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .all(|value| value.as_bytes().eq_ignore_ascii_case(b"identity"));
    if !media_type.is_some_and(|value| value.eq_ignore_ascii_case(AUDIO_FORMAT)) || !identity {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ErrorCategory::InvalidRequest,
        ));
    }
    let too_large = (StatusCode::PAYLOAD_TOO_LARGE, ErrorCategory::InvalidRequest);
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_AUDIO_BYTES)
    {
        return Err(too_large);
    }
    let read = async {
        let mut audio = Vec::new();
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| canceled)?;
            if audio.len() + chunk.len() > MAX_AUDIO_BYTES {
                return Err(too_large);
            }
            audio.extend_from_slice(&chunk);
        }
        Ok(audio)
    };
    let audio = tokio::time::timeout(UPLOAD_TIMEOUT, read)
        .await
        .map_err(|_| canceled)??;
    // Every MPEG-4 file starts with an `ftyp` box.
    if audio.get(4..8) != Some(b"ftyp") {
        return Err((StatusCode::BAD_REQUEST, ErrorCategory::InvalidRequest));
    }
    Ok(Bytes::from(audio))
}

pub(crate) async fn transcription_handler(
    State(service): State<Arc<AiService>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let result = match read_audio(&headers, body).await {
        Ok(audio) => match service.transcribe(audio).await {
            Ok(transcript) => Json(transcript).into_response(),
            Err(category) => error_response(category),
        },
        Err((status, category)) => (status, error_response(category)).into_response(),
    };
    crate::http_admission::no_store(result)
}

#[cfg(test)]
mod tests;

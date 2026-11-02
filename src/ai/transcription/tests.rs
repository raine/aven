use super::*;
use crate::ai::tests::{PendingBackend, ready_service};
use axum::routing::post;
use std::sync::atomic::{AtomicUsize, Ordering};

const AUDIO: &[u8] = b"\0\0\0\x18ftypM4A \0\0\0\0fixture-audio-marker";

fn login_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("chatgpt")).unwrap();
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    fs::write(
        dir.path().join("chatgpt/auth.json"),
        serde_json::json!({
            "access_token": "fixture-access-token",
            "account_id": "fixture-account",
            "expires_at": expires_at,
        })
        .to_string(),
    )
    .unwrap();
    dir
}

fn service(state: BackendState, provider: &str) -> AiService {
    AiService {
        state,
        provider: Some(provider.to_string()),
        timeout: Duration::from_secs(5),
        requests_per_minute: 60,
        ..ready_service()
    }
}

fn chatgpt_state(private_dir: &Path) -> BackendState {
    let config = AiConfig {
        provider: Some("chatgpt".into()),
        model: Some("fixture-model".into()),
        ..AiConfig::default()
    };
    AiService::build_state(&config, private_dir)
}

fn chatgpt_service(private_dir: &Path, upstream: &str) -> AiService {
    let mut state = chatgpt_state(private_dir);
    let BackendState::Ready {
        transcriber: Some(transcriber),
        ..
    } = &mut state
    else {
        panic!("fixture login should make ChatGPT ready");
    };
    transcriber.url = format!("{upstream}/transcribe");
    service(state, "chatgpt")
}

async fn serve(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

async fn serve_service(service: AiService) -> (String, Arc<AiService>) {
    let service = Arc::new(service);
    let router = axum::Router::new()
        .route("/ai/transcriptions", post(transcription_handler))
        .with_state(service.clone());
    (serve(router).await, service)
}

/// Upstream stub that answers each request with the next scripted response
/// and records what it received.
async fn upstream(
    responses: Vec<(u16, &'static str)>,
) -> (String, Arc<Mutex<Vec<(HeaderMap, Bytes)>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let next = Arc::new(AtomicUsize::new(0));
    let recorded = seen.clone();
    let router = axum::Router::new().route(
        "/transcribe",
        post(move |headers: HeaderMap, body: Bytes| async move {
            recorded.lock().unwrap().push((headers, body));
            let (status, body) = responses[next.fetch_add(1, Ordering::SeqCst)];
            (StatusCode::from_u16(status).unwrap(), body)
        }),
    );
    (serve(router).await, seen)
}

async fn post_audio(
    url: &str,
    content_type: &str,
    body: impl Into<reqwest::Body>,
) -> (StatusCode, HeaderMap, Value) {
    let response = reqwest::Client::new()
        .post(format!("{url}/ai/transcriptions"))
        .header("content-type", content_type)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    let headers = response.headers().clone();
    (status, headers, response.json().await.unwrap())
}

fn assert_error(response: &(StatusCode, HeaderMap, Value), status: StatusCode, category: &str) {
    assert_eq!(response.0, status, "{}", response.2);
    assert_eq!(response.1["cache-control"], "no-store");
    assert_eq!(response.2["error"]["category"], category);
}

#[tokio::test]
async fn forwards_audio_with_the_stored_login_and_returns_only_text() {
    let dir = login_dir();
    let (upstream, seen) = upstream(vec![
        (
            200,
            r#"{"text":"buy coffee","asset_pointer":"sediment://file_x","asset_ttl":"30d","asset_format":"m4a"}"#,
        ),
        (200, r#"{"text":""}"#),
    ])
    .await;
    let (url, _) = serve_service(chatgpt_service(dir.path(), &upstream)).await;

    let (status, headers, body) = post_audio(&url, "Audio/MP4; codecs=mp4a", AUDIO).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(body, serde_json::json!({"text": "buy coffee"}));
    let (status, _, body) = post_audio(&url, "audio/mp4", AUDIO).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"text": ""}));

    let (headers, sent) = seen.lock().unwrap()[0].clone();
    assert_eq!(headers["authorization"], "Bearer fixture-access-token");
    assert_eq!(headers["chatgpt-account-id"], "fixture-account");
    assert_eq!(headers["originator"], "aven");
    let boundary = headers["content-type"]
        .to_str()
        .unwrap()
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap()
        .to_string();
    let mut expected = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.m4a\"\r\nContent-Type: audio/mp4\r\n\r\n"
    )
    .into_bytes();
    expected.extend_from_slice(AUDIO);
    expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    assert_eq!(sent, expected);
}

#[tokio::test]
async fn invalid_uploads_are_rejected_without_using_rate_admission() {
    let dir = login_dir();
    let (upstream, seen) = upstream(vec![(200, r#"{"text":"ok"}"#)]).await;
    let mut service = chatgpt_service(dir.path(), &upstream);
    service.requests_per_minute = 1;
    let (url, _) = serve_service(service).await;

    for content_type in ["audio/wav", "application/octet-stream"] {
        let response = post_audio(&url, content_type, AUDIO).await;
        assert_error(
            &response,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
        );
    }
    let response = reqwest::Client::new()
        .post(format!("{url}/ai/transcriptions"))
        .header("content-type", "audio/mp4")
        .header("content-encoding", "gzip")
        .body(AUDIO)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    for body in [&b""[..], b"RIFF\0\0\0\0WAVEfmt "] {
        let response = post_audio(&url, "audio/mp4", body).await;
        assert_error(&response, StatusCode::BAD_REQUEST, "invalid_request");
    }
    assert!(seen.lock().unwrap().is_empty());

    let (status, _, _) = post_audio(&url, "audio/mp4", AUDIO).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn uploads_are_bounded_by_streamed_size_and_time() {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, AUDIO_FORMAT.parse().unwrap());
    let chunks = futures::stream::iter(
        (0..5).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![0u8; MAX_AUDIO_BYTES / 4]))),
    );
    let response = read_audio(&headers, Body::from_stream(chunks))
        .await
        .unwrap_err();
    assert_eq!(response.0, StatusCode::PAYLOAD_TOO_LARGE);
    let mut declared = headers.clone();
    declared.insert(header::CONTENT_LENGTH, (MAX_AUDIO_BYTES + 1).into());
    let response = read_audio(
        &declared,
        Body::from_stream(futures::stream::pending::<Result<Bytes, std::io::Error>>()),
    )
    .await
    .unwrap_err();
    assert_eq!(response.0, StatusCode::PAYLOAD_TOO_LARGE);

    tokio::time::pause();
    let stalled = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(AUDIO))])
        .chain(futures::stream::pending());
    let response = read_audio(&headers, Body::from_stream(stalled))
        .await
        .unwrap_err();
    assert_eq!(response.0, StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test]
async fn upstream_failures_map_to_envelopes_without_upstream_text() {
    let dir = login_dir();
    let cases = [
        (400, StatusCode::BAD_REQUEST, "invalid_request"),
        (
            401,
            StatusCode::SERVICE_UNAVAILABLE,
            "authentication_failure",
        ),
        (
            403,
            StatusCode::SERVICE_UNAVAILABLE,
            "authentication_failure",
        ),
        (429, StatusCode::TOO_MANY_REQUESTS, "rate_limit"),
        (
            500,
            StatusCode::SERVICE_UNAVAILABLE,
            "temporary_provider_failure",
        ),
        (200, StatusCode::BAD_GATEWAY, "malformed_provider_response"),
        (200, StatusCode::BAD_GATEWAY, "malformed_provider_response"),
    ];
    let (upstream, _) = upstream(vec![
        (400, "upstream-secret-marker"),
        (401, "upstream-secret-marker"),
        (403, "upstream-secret-marker"),
        (429, "upstream-secret-marker"),
        (500, r#"{"detail":"upstream-secret-marker"}"#),
        (200, "upstream-secret-marker"),
        (200, r#"{"text":1,"detail":"upstream-secret-marker"}"#),
    ])
    .await;
    let (url, _) = serve_service(chatgpt_service(dir.path(), &upstream)).await;
    for (_, status, category) in cases {
        let response = post_audio(&url, "audio/mp4", AUDIO).await;
        assert_error(&response, status, category);
        assert!(!response.2.to_string().contains("upstream-secret-marker"));
    }
}

#[tokio::test]
async fn unavailable_providers_report_matching_capability_and_error() {
    let dir = login_dir();
    let missing = tempfile::tempdir().unwrap();
    let cases = [
        (
            service(
                BackendState::Ready {
                    backend: Box::new(PendingBackend),
                    schema: SchemaStrategy::Prompted,
                    tools: true,
                    transcriber: None,
                },
                "openai",
            ),
            "unconfigured",
        ),
        (
            service(
                BackendState::Unavailable {
                    status: AvailabilityStatus::AuthenticationRequired,
                    hint: String::new(),
                },
                "openai",
            ),
            "unconfigured",
        ),
        (
            service(chatgpt_state(missing.path()), "chatgpt"),
            "authentication_required",
        ),
    ];
    for (service, status) in cases {
        assert_eq!(
            serde_json::to_value(service.capabilities()).unwrap()["transcription"],
            serde_json::json!({
                "contract": 1,
                "status": status,
                "audio_format": "audio/mp4",
                "max_audio_bytes": 4_194_304,
                "max_duration_seconds": 300,
            })
        );
        let (url, _) = serve_service(service).await;
        let response = post_audio(&url, "audio/mp4", AUDIO).await;
        let category = match status {
            "unconfigured" => "unconfigured",
            _ => "authentication_failure",
        };
        assert_error(&response, StatusCode::SERVICE_UNAVAILABLE, category);
    }

    let ready = service(chatgpt_state(dir.path()), "chatgpt");
    assert_eq!(
        serde_json::to_value(ready.capabilities()).unwrap()["transcription"]["status"],
        "ready"
    );
}

struct DropSignal(tokio::sync::mpsc::UnboundedSender<()>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[tokio::test]
async fn stalled_upstream_times_out_and_disconnect_cancels_it() {
    let dir = login_dir();
    let (entered, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (dropped, mut dropped_rx) = tokio::sync::mpsc::unbounded_channel();
    let router = axum::Router::new().route(
        "/transcribe",
        post(move || {
            let guard = DropSignal(dropped.clone());
            let _ = entered.send(());
            async move {
                let _guard = guard;
                std::future::pending::<()>().await
            }
        }),
    );
    let upstream = serve(router).await;

    let mut timed = chatgpt_service(dir.path(), &upstream);
    timed.timeout = Duration::from_millis(200);
    let (url, _) = serve_service(timed).await;
    let response = post_audio(&url, "audio/mp4", AUDIO).await;
    assert_error(&response, StatusCode::GATEWAY_TIMEOUT, "timeout");
    entered_rx.recv().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), dropped_rx.recv())
        .await
        .unwrap();

    let (url, service) = serve_service(chatgpt_service(dir.path(), &upstream)).await;
    let request = tokio::spawn({
        let url = url.clone();
        async move { post_audio(&url, "audio/mp4", AUDIO).await }
    });
    entered_rx.recv().await.unwrap();
    assert_eq!(service.semaphore.available_permits(), 0);
    let busy = post_audio(&url, "audio/mp4", AUDIO).await;
    assert_error(&busy, StatusCode::TOO_MANY_REQUESTS, "rate_limit");
    request.abort();
    tokio::time::timeout(Duration::from_secs(5), dropped_rx.recv())
        .await
        .unwrap();
    assert_eq!(service.semaphore.available_permits(), 1);
}

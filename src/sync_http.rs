//! Drives core sync client sessions over HTTP.
//!
//! Requests follow no redirects, use no proxy and never decode content;
//! response bodies are read up to one byte past the request's limit, so the
//! engine sees and refuses anything larger. Diagnostics deliberately discard
//! Reqwest URLs and bodies.
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use aven_core::sync::client::{HttpHeader, HttpResponse, Link, PreparedRequest, Session, Step};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

tokio::task_local! {
    /// Transient connectivity feedback for the current foreground operation.
    pub(crate) static NETWORK_STATUS: Arc<Mutex<Option<String>>>;
}

#[derive(Debug)]
pub(crate) struct ConnectivityError(pub(crate) String);

impl std::fmt::Display for ConnectivityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error sync-network {}", self.0)
    }
}
impl std::error::Error for ConnectivityError {}

fn report_network(message: Option<String>) {
    let _ =
        NETWORK_STATUS.try_with(|status| *status.lock().expect("network status lock") = message);
}

/// One Reqwest client that sends session requests.
#[derive(Clone)]
pub struct HttpDriver {
    pub(crate) http: reqwest::Client,
}

impl HttpDriver {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .build()
            .map_err(|_| anyhow::anyhow!("error sync-transport"))?;
        Ok(Self { http })
    }

    /// A single credential-free probe. Any HTTP status proves reachability;
    /// the protocol request remains responsible for validating the server.
    pub(crate) async fn check_reachability(&self, server: &str) -> Result<()> {
        let origin = aven_core::sync::client::server_origin(server)?;
        self.http
            .head(&origin)
            .timeout(CONNECT_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                if matches!(
                    SendFailure::from_reqwest(origin.starts_with("https://"), &error),
                    SendFailure::SecureTransport
                ) {
                    anyhow::anyhow!("error sync-tls")
                } else {
                    anyhow::Error::new(ConnectivityError(
                        crate::sync::error_explanations::connectivity_message(&origin, false),
                    ))
                }
            })?;
        Ok(())
    }

    /// Runs `operation` in a session until it finishes.
    pub async fn run<'a, T, F, Fut>(&self, operation: F) -> Result<T>
    where
        F: FnOnce(Link) -> Fut,
        Fut: Future<Output = Result<T>> + Send + 'a,
    {
        self.drive(Session::new(operation)).await
    }

    /// Answers every step of `session` until it finishes.
    async fn drive<T>(&self, mut session: Session<'_, T>) -> Result<T> {
        let mut network_error = None;
        loop {
            let step = session.next().await.map_err(|error| {
                if crate::sync::error_explanations::codes(&error)
                    .any(|code| code.ends_with("-network"))
                    && let Some(message) = network_error.take()
                {
                    return error.context(ConnectivityError(message));
                }
                error
            })?;
            match step {
                Step::Done(value) => return Ok(value),
                Step::Wait(delay) => tokio::time::sleep(delay).await,
                Step::Request(request) => {
                    let context = request.context;
                    let origin = reqwest::Url::parse(&request.url)?
                        .origin()
                        .ascii_serialization();
                    match self.send(request).await {
                        Ok(response) => {
                            network_error = None;
                            report_network(None);
                            session.accept_response(context, response)?;
                        }
                        Err(SendFailure::Network) => {
                            network_error =
                                Some(crate::sync::error_explanations::connectivity_message(
                                    &origin, false,
                                ));
                            report_network(Some(
                                crate::sync::error_explanations::connectivity_message(
                                    &origin, true,
                                ),
                            ));
                            session.register_transport_failure(context)?;
                        }
                        Err(SendFailure::SecureTransport) => {
                            network_error = None;
                            report_network(None);
                            session.register_secure_transport_failure(context)?;
                        }
                    }
                }
            }
        }
    }

    /// Sends one request of `session` and hands it the outcome.
    pub async fn answer<T>(
        &self,
        session: &mut Session<'_, T>,
        request: PreparedRequest,
    ) -> Result<()> {
        let context = request.context;
        match self.send(request).await {
            Ok(response) => session.accept_response(context, response),
            Err(SendFailure::Network) => session.register_transport_failure(context),
            Err(SendFailure::SecureTransport) => session.register_secure_transport_failure(context),
        }
    }

    async fn send(&self, request: PreparedRequest) -> Result<HttpResponse, SendFailure> {
        let secure = request.url.starts_with("https://");
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| SendFailure::Network)?;
        let mut builder = self
            .http
            .request(method, request.url.as_str())
            .timeout(request.timeout)
            .body(request.body);
        for header in &request.headers {
            let mut value = reqwest::header::HeaderValue::from_str(&header.value)
                .map_err(|_| SendFailure::Network)?;
            if header.name.eq_ignore_ascii_case("authorization") {
                value.set_sensitive(true);
            }
            builder = builder.header(header.name.as_str(), value);
        }
        let mut response = builder
            .send()
            .await
            .map_err(|error| SendFailure::from_reqwest(secure, &error))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some(HttpHeader {
                    name: name.as_str().to_string(),
                    value: value.to_str().ok()?.to_string(),
                })
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| SendFailure::from_reqwest(secure, &error))?
        {
            let room = (request.response_limit + 1).saturating_sub(body.len());
            body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            if body.len() > request.response_limit {
                break;
            }
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[derive(Clone, Copy)]
enum SendFailure {
    Network,
    SecureTransport,
}

impl SendFailure {
    fn from_reqwest(secure: bool, error: &reqwest::Error) -> Self {
        if secure && contains_rustls_error(error) {
            Self::SecureTransport
        } else {
            Self::Network
        }
    }
}

fn contains_rustls_error(error: &(dyn std::error::Error + 'static)) -> bool {
    if error.is::<rustls::Error>() {
        return true;
    }
    if let Some(inner) = error
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::get_ref)
        && contains_rustls_error(inner)
    {
        return true;
    }
    error.source().is_some_and(contains_rustls_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_driver_reuses_a_tcp_connection_across_requests() {
        use axum::serve::ListenerExt;
        use axum::{Router, routing::post};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = accepted.clone();
        let listener = listener.tap_io(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let app = Router::new().route("/", post(|| async { "ok" }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let driver = HttpDriver::new().unwrap();

        for _ in 0..3 {
            let response = driver
                .http
                .post(format!("http://{address}/"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.bytes().await.unwrap(), "ok");
        }

        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn reachability_accepts_http_errors_without_credentials_or_redirects() {
        use axum::{
            Router,
            http::{HeaderMap, StatusCode},
            routing::head,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/",
            head(|headers: HeaderMap| async move {
                assert!(!headers.contains_key("authorization"));
                (StatusCode::FOUND, [("location", "http://127.0.0.1:1/")])
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        HttpDriver::new()
            .unwrap()
            .check_reachability(&origin)
            .await
            .unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn reachability_bounds_a_server_that_never_sends_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let started = std::time::Instant::now();
        let error = tokio::time::timeout(
            Duration::from_secs(8),
            HttpDriver::new().unwrap().check_reachability(&origin),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(started.elapsed() >= CONNECT_TIMEOUT);
        let message = crate::sync::error_explanations::connectivity_error(&error).unwrap();
        assert!(message.contains(&origin));
        assert!(message.contains("Try again"));
        server.abort();
    }

    #[tokio::test]
    async fn retries_report_the_origin_and_clear_feedback_after_a_response() {
        use aven_core::sync::client::enrollment::{Client, Operation};
        use axum::{Router, routing::post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let status = Arc::new(Mutex::new(None));
        let observed = status.clone();
        let server = tokio::spawn(async move {
            // Drop the first request without a response, then accept its retry.
            let (socket, _) = listener.accept().await.unwrap();
            drop(socket);
            let app =
                Router::new().route("/e2ee/enrollment/v1", post(|| async { axum::Json("Done") }));
            axum::serve(listener, app).await.unwrap();
        });
        NETWORK_STATUS
            .scope(status, async {
                HttpDriver::new()
                    .unwrap()
                    .run(|link| async {
                        let client = Client::new(&origin, link)?;
                        let operation = || Operation::Mailbox {
                            vault: [0; 32],
                            handle: [0; 32],
                        };
                        assert!(client.exchange(operation(), None).await.is_err());
                        let message = observed.lock().unwrap().clone().unwrap();
                        assert!(message.contains(&origin));
                        assert!(message.contains("Retrying."));
                        assert!(!message.contains("/e2ee/"));
                        client.exchange(operation(), None).await?;
                        assert!(observed.lock().unwrap().is_none());
                        Ok(())
                    })
                    .await
                    .unwrap();
            })
            .await;
        server.abort();
    }

    #[tokio::test]
    async fn connect_timeout_also_bounds_a_stalled_tls_handshake() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        // No request timeout: only the client's connect timeout can end this.
        let error = tokio::time::timeout(
            Duration::from_secs(8),
            HttpDriver::new().unwrap().http.get(&origin).send(),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.is_timeout());
        server.abort();
    }

    #[test]
    fn finds_rustls_errors_nested_in_io_errors() {
        let tls = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::General("certificate failure".into()),
        );
        let outer = std::io::Error::other(tls);
        assert!(contains_rustls_error(&outer));
        assert!(!contains_rustls_error(&std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused
        )));
    }
}

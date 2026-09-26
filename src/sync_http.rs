//! Drives core sync client sessions over HTTP.
//!
//! Requests follow no redirects, use no proxy and never decode content;
//! response bodies are read up to one byte past the request's limit, so the
//! engine sees and refuses anything larger. Diagnostics deliberately discard
//! Reqwest URLs and bodies.
use std::future::Future;

use anyhow::Result;
use aven_core::sync::client::{HttpHeader, HttpResponse, Link, PreparedRequest, Session, Step};

/// One Reqwest client that sends session requests.
#[derive(Clone)]
pub struct HttpDriver {
    pub(crate) http: reqwest::Client,
}

impl HttpDriver {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .build()
            .map_err(|_| anyhow::anyhow!("error bootstrap-transport"))?;
        Ok(Self { http })
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
    pub async fn drive<T>(&self, mut session: Session<'_, T>) -> Result<T> {
        loop {
            match session.next().await? {
                Step::Done(value) => return Ok(value),
                Step::Wait(delay) => tokio::time::sleep(delay).await,
                Step::Request(request) => {
                    let context = request.context;
                    match self.send(request).await {
                        Some(response) => session.accept_response(context, response)?,
                        None => session.register_transport_failure(context)?,
                    }
                }
            }
        }
    }

    async fn send(&self, request: PreparedRequest) -> Option<HttpResponse> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).ok()?;
        let mut builder = self
            .http
            .request(method, request.url.as_str())
            .timeout(request.timeout)
            .body(request.body);
        for header in &request.headers {
            let mut value = reqwest::header::HeaderValue::from_str(&header.value).ok()?;
            if header.name.eq_ignore_ascii_case("authorization") {
                value.set_sensitive(true);
            }
            builder = builder.header(header.name.as_str(), value);
        }
        let mut response = builder.send().await.ok()?;
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
        while let Some(chunk) = response.chunk().await.ok()? {
            let room = (request.response_limit + 1).saturating_sub(body.len());
            body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            if body.len() > request.response_limit {
                break;
            }
        }
        Some(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

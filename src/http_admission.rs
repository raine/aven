use std::{future::Future, time::Duration};

use aven_core::sync::seed_claim::Secret;
use axum::{
    body::{Bytes, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tokio::sync::Semaphore;

pub(crate) enum Outcome<T> {
    Dispatched(T),
    PermitTimeout,
    DispatchTimeout,
}

/// Waits for admission and dispatches within one deadline.
pub(crate) async fn dispatch<T>(
    admission: &Semaphore,
    timeout: Duration,
    future: impl Future<Output = T>,
) -> Outcome<T> {
    let deadline = tokio::time::Instant::now() + timeout;
    let permit = match tokio::time::timeout_at(deadline, admission.acquire()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) | Err(_) => return Outcome::PermitTimeout,
    };
    let outcome = match tokio::time::timeout_at(deadline, future).await {
        Ok(value) => Outcome::Dispatched(value),
        Err(_) => Outcome::DispatchTimeout,
    };
    drop(permit);
    outcome
}

pub(crate) fn mark_busy(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
}

/// Whether the request declares an uncompressed JSON body.
pub(crate) fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        == Some("application/json")
        && !headers.contains_key(header::CONTENT_ENCODING)
}

/// Parses an `Authorization: Bearer <64 lowercase hex digits>` credential.
pub(crate) fn bearer(value: &HeaderValue) -> Option<Secret> {
    value
        .to_str()
        .ok()
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .and_then(|s| hex::decode(s).ok())
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
        .map(Secret::new)
}

/// Reads the whole body, refusing bodies over `limit` bytes.
pub(crate) async fn body(request: Request, limit: usize) -> Option<Bytes> {
    to_bytes(request.into_body(), limit).await.ok()
}

/// A JSON response, or `None` when the encoded reply exceeds `limit` bytes.
pub(crate) fn json(reply: &impl Serialize, limit: usize) -> Option<Response> {
    serde_json::to_vec(reply)
        .ok()
        .filter(|bytes| bytes.len() <= limit)
        .map(|bytes| ([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

pub(crate) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test(start_paused = true)]
    async fn permit_timeout_never_starts_dispatch() {
        let admission = Arc::new(Semaphore::new(1));
        let permit = admission.clone().acquire_owned().await.unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let dispatched = started.clone();
        let task = tokio::spawn(async move {
            dispatch(&admission, Duration::from_secs(30), async move {
                dispatched.store(true, Ordering::SeqCst);
            })
            .await
        });
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(matches!(task.await.unwrap(), Outcome::PermitTimeout));
        assert!(!started.load(Ordering::SeqCst));
        drop(permit);
    }

    #[tokio::test(start_paused = true)]
    async fn dispatch_timeout_is_distinct_after_dispatch_starts() {
        let admission = Semaphore::new(1);
        let started = Arc::new(AtomicBool::new(false));
        let dispatched = started.clone();
        let task = tokio::spawn(async move {
            dispatch(&admission, Duration::from_secs(30), async move {
                dispatched.store(true, Ordering::SeqCst);
                std::future::pending::<()>().await;
            })
            .await
        });
        tokio::task::yield_now().await;
        assert!(started.load(Ordering::SeqCst));
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(matches!(task.await.unwrap(), Outcome::DispatchTimeout));
    }
}

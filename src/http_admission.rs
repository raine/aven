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

/// Requests collecting or holding a body per router. Callers beyond it are
/// refused as busy rather than queued, which also bounds operation waiters.
const INGRESS_LIMIT: usize = 16;
/// Longest a request may take to deliver its complete body.
pub(crate) const BODY_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) enum Outcome<T> {
    Dispatched(T),
    PermitTimeout,
    DispatchTimeout,
}

/// Admission for one router. Bodies are collected under a bounded ingress
/// pool, so a slow body never holds one of the scarce operation permits.
pub(crate) struct Admission {
    ingress: Semaphore,
    operations: Semaphore,
}

impl Admission {
    pub(crate) fn new(operations: usize) -> Self {
        Self {
            ingress: Semaphore::new(INGRESS_LIMIT),
            operations: Semaphore::new(operations),
        }
    }

    #[cfg(test)]
    pub(crate) async fn hold_operation(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.operations.acquire().await.unwrap()
    }

    #[cfg(test)]
    pub(crate) fn available_operations(&self) -> usize {
        self.operations.available_permits()
    }

    #[cfg(test)]
    pub(crate) fn available_ingress(&self) -> usize {
        self.ingress.available_permits()
    }
}

/// Collects a body of at most `limit` bytes within [`BODY_TIMEOUT`], then
/// waits for an operation permit and runs `operate` within one deadline.
/// `operate` receives `None` for a body over `limit` or otherwise unreadable.
pub(crate) async fn dispatch<T, F>(
    admission: &Admission,
    timeout: Duration,
    request: Request,
    limit: usize,
    operate: impl FnOnce(HeaderMap, Option<Bytes>) -> F,
) -> Outcome<T>
where
    F: Future<Output = T>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let Ok(_ingress) = admission.ingress.try_acquire() else {
        return Outcome::PermitTimeout;
    };
    let (parts, body) = request.into_parts();
    let body_deadline = deadline.min(tokio::time::Instant::now() + BODY_TIMEOUT);
    let Ok(bytes) = tokio::time::timeout_at(body_deadline, to_bytes(body, limit)).await else {
        return Outcome::DispatchTimeout;
    };
    let permit = match tokio::time::timeout_at(deadline, admission.operations.acquire()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) | Err(_) => return Outcome::PermitTimeout,
    };
    let outcome = match tokio::time::timeout_at(deadline, operate(parts.headers, bytes.ok())).await
    {
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
    use axum::body::{Body, HttpBody};
    use std::{
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll},
    };

    /// A body whose sender never delivers a byte.
    struct Stalled;

    impl HttpBody for Stalled {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
            Poll::Pending
        }
    }

    fn request(body: Body) -> Request {
        Request::builder().body(body).unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn permit_timeout_never_starts_dispatch() {
        let admission = Arc::new(Admission::new(1));
        let held = admission.clone();
        let permit = held.hold_operation().await;
        let started = Arc::new(AtomicBool::new(false));
        let dispatched = started.clone();
        let task = tokio::spawn(async move {
            dispatch(
                &admission,
                Duration::from_secs(30),
                request(Body::empty()),
                16,
                |_, _| async move {
                    dispatched.store(true, Ordering::SeqCst);
                },
            )
            .await
        });
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(matches!(task.await.unwrap(), Outcome::PermitTimeout));
        assert!(!started.load(Ordering::SeqCst));
        drop(permit);
    }

    #[tokio::test(start_paused = true)]
    async fn dispatch_timeout_is_distinct_after_dispatch_starts() {
        let admission = Admission::new(1);
        let started = Arc::new(AtomicBool::new(false));
        let dispatched = started.clone();
        let task = tokio::spawn(async move {
            dispatch(
                &admission,
                Duration::from_secs(30),
                request(Body::empty()),
                16,
                |_, _| async move {
                    dispatched.store(true, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                },
            )
            .await
        });
        tokio::task::yield_now().await;
        assert!(started.load(Ordering::SeqCst));
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(matches!(task.await.unwrap(), Outcome::DispatchTimeout));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_body_holds_no_operation_permit_and_times_out() {
        let admission = Arc::new(Admission::new(1));
        let stalled = {
            let admission = admission.clone();
            tokio::spawn(async move {
                dispatch(
                    &admission,
                    Duration::from_secs(30),
                    request(Body::new(Stalled)),
                    16,
                    |_, _| async { unreachable!("a stalled body never dispatches") },
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(admission.available_ingress(), INGRESS_LIMIT - 1);
        assert_eq!(admission.available_operations(), 1);
        // A complete request is served at once while the body stalls.
        let served = dispatch(
            &admission,
            Duration::from_secs(30),
            request(Body::from("{}")),
            16,
            |_, bytes| async move { bytes },
        )
        .await;
        assert!(matches!(served, Outcome::Dispatched(Some(bytes)) if bytes == "{}"));
        tokio::time::advance(BODY_TIMEOUT).await;
        assert!(matches!(stalled.await.unwrap(), Outcome::DispatchTimeout));
        assert_eq!(admission.available_ingress(), INGRESS_LIMIT);
        assert_eq!(admission.available_operations(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn full_ingress_is_refused_without_queueing() {
        let admission = Admission::new(1);
        let held = admission
            .ingress
            .try_acquire_many(INGRESS_LIMIT as u32)
            .unwrap();
        let outcome = dispatch(
            &admission,
            Duration::from_secs(30),
            request(Body::from("{}")),
            16,
            |_, _| async { unreachable!("refused before collecting the body") },
        )
        .await;
        assert!(matches!(outcome, Outcome::PermitTimeout));
        drop(held);
    }

    #[tokio::test]
    async fn oversized_body_reaches_the_operation_as_none() {
        let admission = Admission::new(1);
        let outcome = dispatch(
            &admission,
            Duration::from_secs(30),
            request(Body::from("x".repeat(17))),
            16,
            |_, bytes| async move { bytes.is_none() },
        )
        .await;
        assert!(matches!(outcome, Outcome::Dispatched(true)));
    }
}

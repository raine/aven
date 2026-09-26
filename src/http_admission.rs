use std::{future::Future, time::Duration};

use aven_core::sync::seed_claim::{Secret, membership};
use axum::{
    body::{Bytes, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::fmt;
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

/// A refusal with a `{"error":"<code>"}` body. Statuses follow meaning: 400
/// malformed, 401 unauthenticated, 403 forbidden, 408 timeout, 409 stale or
/// conflicting, 413 too large, 415 wrong content type, 500 server fault and
/// 503 busy with `Retry-After`. Clients act on the code, not the status.
pub(crate) fn refusal(status: StatusCode, code: &str) -> Response {
    no_store(
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "error": code }).to_string(),
        )
            .into_response(),
    )
}

/// A request refused before or during its operation, carried through
/// `anyhow` so operation code can return it with `?`.
#[derive(Debug)]
pub(crate) struct Refusal {
    status: StatusCode,
    code: &'static str,
}

impl Refusal {
    pub(crate) fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error {}", self.code)
    }
}

impl std::error::Error for Refusal {}

/// Error codes of one router, each prefixed with its family name.
pub(crate) struct Codes {
    /// 415: the body is not uncompressed JSON.
    pub(crate) content_type: &'static str,
    /// 401: a required bearer credential is missing or malformed.
    pub(crate) credential: &'static str,
    /// 413: the body exceeds its limit.
    pub(crate) limit: &'static str,
    /// 400: the body does not parse as a request.
    pub(crate) malformed: &'static str,
    /// 400: the operation refused a well-formed request.
    pub(crate) refused: &'static str,
    /// 403: the credential may not perform the operation.
    pub(crate) unauthorized: &'static str,
    /// 408: the body or the operation did not finish in time.
    pub(crate) timeout: &'static str,
    /// 500: storage failed or the reply exceeded its limit.
    pub(crate) server_error: &'static str,
    /// 503: every permit is taken; retry after `Retry-After`.
    pub(crate) busy: &'static str,
}

/// The [`Codes`] of a router family, e.g. `codes!("enrollment")`.
macro_rules! codes {
    ($family:literal) => {
        $crate::http_admission::Codes {
            content_type: concat!($family, "-content-type"),
            credential: concat!($family, "-credential"),
            limit: concat!($family, "-limit"),
            malformed: concat!($family, "-malformed"),
            refused: concat!($family, "-refused"),
            unauthorized: concat!($family, "-unauthorized"),
            timeout: concat!($family, "-timeout"),
            server_error: concat!($family, "-server-error"),
            busy: concat!($family, "-busy"),
        }
    };
}
pub(crate) use codes;

impl Codes {
    /// An uncompressed JSON body within the router's collection limit.
    pub(crate) fn json_body(
        &self,
        headers: &HeaderMap,
        bytes: Option<Bytes>,
    ) -> Result<Bytes, Refusal> {
        if !is_json(headers) {
            return Err(Refusal::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                self.content_type,
            ));
        }
        bytes.ok_or(Refusal::new(StatusCode::PAYLOAD_TOO_LARGE, self.limit))
    }

    /// A present `Authorization` header that must be a valid bearer.
    pub(crate) fn optional_bearer(&self, headers: &HeaderMap) -> Result<Option<Secret>, Refusal> {
        headers
            .get(header::AUTHORIZATION)
            .map(|value| bearer(value).ok_or_else(|| self.missing_credential()))
            .transpose()
    }

    pub(crate) fn bearer(&self, headers: &HeaderMap) -> Result<Secret, Refusal> {
        self.optional_bearer(headers)?
            .ok_or_else(|| self.missing_credential())
    }

    pub(crate) fn missing_credential(&self) -> Refusal {
        Refusal::new(StatusCode::UNAUTHORIZED, self.credential)
    }

    pub(crate) fn too_large(&self) -> Refusal {
        Refusal::new(StatusCode::PAYLOAD_TOO_LARGE, self.limit)
    }

    pub(crate) fn parse<T: serde::de::DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, Refusal> {
        serde_json::from_slice(bytes)
            .map_err(|_| Refusal::new(StatusCode::BAD_REQUEST, self.malformed))
    }
}

/// The refusal for an operation error. Errors the operation did not
/// classify are refusals of a well-formed request, except storage faults.
pub(crate) fn operation_refusal(codes: &Codes, error: &anyhow::Error) -> Response {
    if let Some(known) = error.downcast_ref::<Refusal>() {
        refusal(known.status, known.code)
    } else if error.downcast_ref::<membership::StaleContext>().is_some() {
        refusal(StatusCode::CONFLICT, "membership-stale")
    } else if error.downcast_ref::<membership::Unauthorized>().is_some() {
        refusal(StatusCode::FORBIDDEN, codes.unauthorized)
    } else if aven_core::db::is_storage_error(error) {
        refusal(StatusCode::INTERNAL_SERVER_ERROR, codes.server_error)
    } else {
        refusal(StatusCode::BAD_REQUEST, codes.refused)
    }
}

/// The response for an admission outcome whose operation produced one.
pub(crate) fn respond(codes: &Codes, outcome: Outcome<Response>) -> Response {
    match outcome {
        Outcome::Dispatched(response) => no_store(response),
        Outcome::DispatchTimeout => refusal(StatusCode::REQUEST_TIMEOUT, codes.timeout),
        Outcome::PermitTimeout => {
            let mut response = refusal(StatusCode::SERVICE_UNAVAILABLE, codes.busy);
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            response
        }
    }
}

/// A JSON reply, or a server fault when it exceeds `limit` bytes.
pub(crate) fn reply(codes: &Codes, reply: &impl Serialize, limit: usize) -> Response {
    json(reply, limit)
        .unwrap_or_else(|| refusal(StatusCode::INTERNAL_SERVER_ERROR, codes.server_error))
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

//! Sans-IO HTTP exchanges between the client engine and its host.
//!
//! A [`Session`] runs one engine operation. The host repeatedly calls
//! [`Session::next`]: it sends each [`Step::Request`] exactly as prepared and
//! answers it with [`Session::accept_response`] or
//! [`Session::register_transport_failure`], waits out each [`Step::Wait`],
//! and stops at [`Step::Done`] or an error. Retry and polling decisions stay
//! in the engine, which expresses them as waits and repeated requests.
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Poll;
use std::time::Duration;

use anyhow::{Result, bail};

/// Timeout for one request, from sending to the last response byte.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(35);

/// One HTTP header.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for HttpHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.name.eq_ignore_ascii_case("authorization") {
            write!(f, "{}: [REDACTED]", self.name)
        } else {
            write!(f, "{}: {}", self.name, self.value)
        }
    }
}

/// Names the request a response or failure answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestContext {
    id: u64,
}

/// A request the host sends exactly as prepared.
///
/// The host follows no redirects, uses no proxy or cookies, adds no
/// `Accept-Encoding` and never decodes content. It reads at most
/// `response_limit + 1` body bytes and passes them on; the engine refuses
/// oversized bodies itself. A connection, timeout or body read failure is a
/// transport failure. The `Authorization` header is a credential.
pub struct PreparedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
    pub timeout: Duration,
    pub response_limit: usize,
    pub context: RequestContext,
}

impl fmt::Debug for PreparedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .field("timeout", &self.timeout)
            .field("response_limit", &self.response_limit)
            .field("context", &self.context)
            .finish()
    }
}

/// A complete HTTP response of any status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// The first header named `name`, ignoring case.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }

    pub(crate) fn has_header(&self, name: &str) -> bool {
        self.header(name).is_some()
    }

    pub(crate) fn is_json(&self) -> bool {
        self.header("content-type") == Some("application/json")
    }

    pub(crate) fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.parse().ok()
    }

    pub(crate) fn retry_after(&self) -> Option<u64> {
        self.header("retry-after")?.parse().ok()
    }
}

/// What the host does next.
#[derive(Debug)]
pub enum Step<T> {
    /// Send this request, then answer it before calling `next` again.
    Request(PreparedRequest),
    /// Wait this long, then call `next` again.
    Wait(Duration),
    /// The operation finished.
    Done(T),
}

/// The request never produced a complete response.
#[derive(Debug)]
pub(crate) struct TransportFailure;

enum Outgoing {
    Request(PreparedRequest),
    Wait(Duration),
}

#[derive(Default)]
enum Pending {
    #[default]
    None,
    Request(u64),
    Answered(u64, Result<HttpResponse, TransportFailure>),
    Waiting,
    Waited,
}

#[derive(Default)]
struct LinkState {
    next_id: u64,
    outgoing: Option<Outgoing>,
    pending: Pending,
}

/// The engine's connection to its host within one session.
#[derive(Clone, Default)]
pub struct Link {
    state: Arc<Mutex<LinkState>>,
}

impl Link {
    fn state(&self) -> MutexGuard<'_, LinkState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Hands `request` to the host and waits for its answer.
    pub(crate) async fn send(
        &self,
        method: &str,
        url: &url::Url,
        headers: Vec<HttpHeader>,
        body: Vec<u8>,
        response_limit: usize,
    ) -> Result<HttpResponse, TransportFailure> {
        let id = {
            let mut state = self.state();
            let id = state.next_id;
            state.next_id += 1;
            state.outgoing = Some(Outgoing::Request(PreparedRequest {
                method: method.to_string(),
                url: url.to_string(),
                headers,
                body,
                timeout: REQUEST_TIMEOUT,
                response_limit,
                context: RequestContext { id },
            }));
            state.pending = Pending::Request(id);
            id
        };
        poll_fn(|_| {
            let mut state = self.state();
            match std::mem::take(&mut state.pending) {
                Pending::Answered(answered, result) if answered == id => Poll::Ready(result),
                other => {
                    state.pending = other;
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// Lets the host wait `delay` before the engine continues.
    pub(crate) async fn wait(&self, delay: Duration) {
        {
            let mut state = self.state();
            state.outgoing = Some(Outgoing::Wait(delay));
            state.pending = Pending::Waiting;
        }
        poll_fn(|_| {
            let mut state = self.state();
            if matches!(state.pending, Pending::Waited) {
                state.pending = Pending::None;
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }

    fn answer(
        &self,
        context: RequestContext,
        result: Result<HttpResponse, TransportFailure>,
    ) -> Result<()> {
        let mut state = self.state();
        match state.pending {
            Pending::Request(id) if id == context.id && state.outgoing.is_none() => {
                state.pending = Pending::Answered(id, result);
                Ok(())
            }
            _ => bail!("error sync-exchange-unexpected-answer"),
        }
    }
}

type Operation<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// One engine operation driven by its host.
pub struct Session<'a, T> {
    operation: Option<Operation<'a, T>>,
    link: Link,
}

impl<'a, T> Session<'a, T> {
    /// Runs `operation` with a link to this session's host.
    pub fn new<F, Fut>(operation: F) -> Self
    where
        F: FnOnce(Link) -> Fut,
        Fut: Future<Output = Result<T>> + Send + 'a,
    {
        let link = Link::default();
        Self {
            operation: Some(Box::pin(operation(link.clone()))),
            link,
        }
    }

    /// Runs the operation until it needs the host or finishes. An error
    /// finishes the session.
    pub async fn next(&mut self) -> Result<Step<T>> {
        {
            let mut state = self.link.state();
            match state.pending {
                Pending::Request(_) if state.outgoing.is_none() => {
                    bail!("error sync-exchange-unanswered")
                }
                Pending::Waiting if state.outgoing.is_none() => state.pending = Pending::Waited,
                _ => {}
            }
        }
        let Some(operation) = self.operation.as_mut() else {
            bail!("error sync-session-finished");
        };
        let link = &self.link;
        let finished = poll_fn(|context| {
            if let Poll::Ready(result) = operation.as_mut().poll(context) {
                return Poll::Ready(Some(result));
            }
            if link.state().outgoing.is_some() {
                return Poll::Ready(None);
            }
            Poll::Pending
        })
        .await;
        if let Some(result) = finished {
            self.operation = None;
            return result.map(Step::Done);
        }
        Ok(match self.link.state().outgoing.take() {
            Some(Outgoing::Request(request)) => Step::Request(request),
            Some(Outgoing::Wait(delay)) => Step::Wait(delay),
            None => unreachable!("outgoing step observed above"),
        })
    }

    /// Answers the outstanding request with the response the host received.
    pub fn accept_response(
        &mut self,
        context: RequestContext,
        response: HttpResponse,
    ) -> Result<()> {
        self.link.answer(context, Ok(response))
    }

    /// Answers the outstanding request that produced no complete response.
    pub fn register_transport_failure(&mut self, context: RequestContext) -> Result<()> {
        self.link.answer(context, Err(TransportFailure))
    }
}

/// Backoff before retrying a busy server; never shorter than its
/// `Retry-After`, capped at two seconds.
pub(crate) fn busy_retry_delay(attempt: usize, retry_after: u64) -> Duration {
    let base_ms = 50_u64 << attempt.min(5);
    let mut random = [0_u8; 2];
    let _ = getrandom::fill(&mut random);
    let jitter_ms = u16::from_le_bytes(random) as u64 % (base_ms / 2 + 1);
    Duration::from_millis(base_ms + jitter_ms).max(Duration::from_secs(retry_after.min(2)))
}

/// A bearer credential header.
pub(crate) fn bearer(secret: &crate::sync::seed_claim::Secret) -> HttpHeader {
    HttpHeader {
        name: "authorization".into(),
        value: format!("Bearer {}", hex::encode(secret.expose())),
    }
}

pub(crate) fn json_content() -> HttpHeader {
    HttpHeader {
        name: "content-type".into(),
        value: "application/json".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn two_requests(link: Link) -> Result<Vec<u16>> {
        let url = url::Url::parse("https://sync.example.com/x").unwrap();
        let mut statuses = Vec::new();
        for _ in 0..2 {
            match link
                .send("POST", &url, vec![json_content()], vec![1], 8)
                .await
            {
                Ok(response) => statuses.push(response.status),
                Err(TransportFailure) => statuses.push(0),
            }
            link.wait(Duration::from_millis(5)).await;
        }
        Ok(statuses)
    }

    #[tokio::test]
    async fn session_yields_requests_and_waits_in_order() {
        let mut session = Session::new(two_requests);
        let Step::Request(first) = session.next().await.unwrap() else {
            panic!("expected a request");
        };
        assert_eq!(first.method, "POST");
        assert_eq!(first.timeout, REQUEST_TIMEOUT);
        assert!(session.next().await.is_err(), "an unanswered request stops");
        let response = HttpResponse {
            status: 204,
            headers: vec![],
            body: vec![],
        };
        session.accept_response(first.context, response).unwrap();
        assert!(matches!(
            session.next().await.unwrap(),
            Step::Wait(delay) if delay == Duration::from_millis(5)
        ));
        let Step::Request(second) = session.next().await.unwrap() else {
            panic!("expected a request");
        };
        assert!(
            session
                .accept_response(
                    first.context,
                    HttpResponse {
                        status: 200,
                        headers: vec![],
                        body: vec![]
                    }
                )
                .is_err(),
            "a stale context is refused"
        );
        session.register_transport_failure(second.context).unwrap();
        assert!(matches!(session.next().await.unwrap(), Step::Wait(_)));
        assert!(
            matches!(session.next().await.unwrap(), Step::Done(statuses) if statuses == [204, 0])
        );
        assert!(session.next().await.is_err());
    }

    #[test]
    fn credentials_stay_out_of_debug_output() {
        let secret = crate::sync::seed_claim::Secret::new([7; 32]);
        let header = bearer(&secret);
        assert!(!format!("{header:?}").contains(&hex::encode([7; 32])));
    }
}

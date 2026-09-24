use std::{future::Future, time::Duration};

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

pub(crate) fn mark_busy(response: &mut axum::response::Response) {
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("1"),
    );
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

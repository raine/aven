use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use tokio::sync::Semaphore;

const MAX_ATTACHMENT_WORKERS: usize = 2;
const MAX_PREVIEW_WORKERS: usize = 2;

static ATTACHMENT_WORKERS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_ATTACHMENT_WORKERS)));
static PREVIEW_WORKERS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_PREVIEW_WORKERS)));

pub(crate) async fn run<F, T>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    run_with_pool(ATTACHMENT_WORKERS.clone(), work).await
}

pub(crate) async fn run_preview<F, T>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    run_with_pool(PREVIEW_WORKERS.clone(), work).await
}

async fn run_with_pool<F, T>(workers: Arc<Semaphore>, work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let permit = workers
        .acquire_owned()
        .await
        .context("image worker pool closed")?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .context("image worker failed")?
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::MAX_PREVIEW_WORKERS;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn preview_capacity_does_not_delay_attachment_work() {
        let mut started = Vec::new();
        let mut previews = Vec::new();
        for _ in 0..MAX_PREVIEW_WORKERS {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            previews.push(tokio::spawn(async move {
                super::run_preview(move || {
                    let _ = started_tx.send(());
                    std::thread::sleep(Duration::from_millis(150));
                    Ok(())
                })
                .await
            }));
            started.push(started_rx);
        }
        for started in started {
            started.await.unwrap();
        }

        tokio::time::timeout(Duration::from_millis(100), super::run(|| Ok(())))
            .await
            .expect("attachment pool should remain available")
            .unwrap();
        for preview in previews {
            preview.await.unwrap().unwrap();
        }
    }
}

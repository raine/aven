use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use tokio::sync::Semaphore;

const MAX_IMAGE_WORKERS: usize = 2;

static IMAGE_WORKERS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_IMAGE_WORKERS)));

pub async fn run<F, T>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let permit = IMAGE_WORKERS
        .clone()
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

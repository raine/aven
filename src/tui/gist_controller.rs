use anyhow::{Context, Result};
use tokio::task::JoinHandle;

pub(super) struct GistController {
    task: Option<JoinHandle<Result<String>>>,
}

impl GistController {
    pub(super) fn new() -> Self {
        Self { task: None }
    }

    pub(super) fn start(
        &mut self,
        markdown: String,
        filename: String,
        description: String,
    ) -> bool {
        if self.task.is_some() {
            return false;
        }
        self.task = Some(tokio::task::spawn_blocking(move || {
            crate::tui::platform::create_secret_gist(&markdown, &filename, &description)
        }));
        true
    }

    pub(super) fn work_pending(&self) -> bool {
        self.task.is_some()
    }

    #[cfg(test)]
    pub(super) fn set_test_task(&mut self, task: JoinHandle<Result<String>>) {
        self.task = Some(task);
    }

    pub(super) async fn poll(&mut self) -> Option<Result<String>> {
        if !self.task.as_ref().is_some_and(JoinHandle::is_finished) {
            return None;
        }
        let task = self.task.take().expect("finished gist task");
        Some(match task.await {
            Ok(result) => result,
            Err(error) => Err(error).context("gist creation task stopped"),
        })
    }
}

impl Drop for GistController {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn controller_keeps_running_gist_work_pending() {
        let mut controller = GistController::new();
        controller.task = Some(tokio::spawn(std::future::pending()));

        assert!(controller.work_pending());
        assert!(controller.poll().await.is_none());
        assert!(controller.work_pending());
    }

    #[tokio::test]
    async fn controller_polls_finished_gist_work() {
        let mut controller = GistController::new();
        controller.task = Some(tokio::spawn(async {
            Ok("https://gist.example/1".to_string())
        }));
        tokio::task::yield_now().await;

        assert!(controller.work_pending());
        assert_eq!(
            controller.poll().await.unwrap().unwrap(),
            "https://gist.example/1"
        );
        assert!(!controller.work_pending());
    }
}

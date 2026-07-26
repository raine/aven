use std::path::PathBuf;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::config::AppConfig;
use crate::ids::WorkspaceId;
use crate::task_intake::TaskIntakeResult;
use crate::tui::natural_add_runtime::spawn_add_task_only_natural;
use crate::tui::store::TuiStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NaturalRetry {
    AddTask,
    Dialog,
}

struct PendingTaskIntake {
    handle: JoinHandle<Result<TaskIntakeResult>>,
    retry: NaturalRetry,
    value: String,
    create_on_success: bool,
}

struct ReadyTaskIntake {
    outcome: Result<TaskIntakeResult>,
    retry: NaturalRetry,
    value: String,
    create_on_success: bool,
}

enum IntakeState {
    Idle,
    Pending(PendingTaskIntake),
    Ready(Box<ReadyTaskIntake>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IntakeWorkView {
    Idle,
    Pending,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IntakeViewState<'a> {
    pub(super) add_task_only: bool,
    pub(super) message: Option<&'a str>,
    pub(super) work: IntakeWorkView,
}

pub(super) enum IntakePoll {
    Unchanged,
    Ready { failed: bool },
    Completed(Box<IntakeCompletion>),
}

pub(super) struct IntakeCompletion {
    pub(super) outcome: Result<TaskIntakeResult>,
    pub(super) retry: NaturalRetry,
    pub(super) value: String,
    pub(super) create_on_success: bool,
}

pub(super) struct IntakeController {
    state: IntakeState,
    add_task_only: bool,
    message: Option<String>,
    db_path: Option<PathBuf>,
    config: AppConfig,
}

impl IntakeController {
    pub(super) fn new() -> Self {
        Self {
            state: IntakeState::Idle,
            add_task_only: false,
            message: None,
            db_path: None,
            config: AppConfig::default(),
        }
    }

    pub(super) fn set_config(&mut self, config: AppConfig) {
        self.config = config;
    }

    pub(super) fn set_db_path(&mut self, db_path: PathBuf) {
        self.db_path = Some(db_path);
    }

    pub(super) fn enter_add_task_only(&mut self, config: AppConfig) {
        self.add_task_only = true;
        self.config = config;
    }

    pub(super) fn start(
        &mut self,
        store: &TuiStore,
        raw: String,
        project: Option<String>,
        retry: NaturalRetry,
        value: String,
        create_on_success: bool,
    ) {
        let handle = store.spawn_task_intake(self.config.agent.task_intake.clone(), raw, project);
        self.start_worker(handle, retry, value, create_on_success);
    }

    fn start_worker(
        &mut self,
        handle: JoinHandle<Result<TaskIntakeResult>>,
        retry: NaturalRetry,
        value: String,
        create_on_success: bool,
    ) {
        self.cancel();
        self.state = IntakeState::Pending(PendingTaskIntake {
            handle,
            retry,
            value,
            create_on_success,
        });
    }

    #[cfg(test)]
    pub(super) fn start_handle(
        &mut self,
        handle: JoinHandle<Result<TaskIntakeResult>>,
        retry: NaturalRetry,
        value: String,
        create_on_success: bool,
    ) {
        self.start_worker(handle, retry, value, create_on_success);
    }

    pub(super) fn start_detached(
        &self,
        raw: &str,
        workspace_id: &WorkspaceId,
        project: Option<&str>,
        allow_existing_project: bool,
    ) -> Result<()> {
        spawn_add_task_only_natural(
            raw,
            workspace_id,
            self.db_path.as_deref(),
            project,
            allow_existing_project,
        )
    }

    pub(super) async fn poll(&mut self) -> Result<IntakePoll> {
        if matches!(self.state, IntakeState::Ready(_)) {
            let IntakeState::Ready(ready) = std::mem::replace(&mut self.state, IntakeState::Idle)
            else {
                unreachable!("intake state checked above");
            };
            return Ok(IntakePoll::Completed(Box::new(IntakeCompletion {
                outcome: ready.outcome,
                retry: ready.retry,
                value: ready.value,
                create_on_success: ready.create_on_success,
            })));
        }

        let IntakeState::Pending(pending) = &self.state else {
            return Ok(IntakePoll::Unchanged);
        };
        if !pending.handle.is_finished() {
            return Ok(IntakePoll::Unchanged);
        }

        let IntakeState::Pending(pending) = std::mem::replace(&mut self.state, IntakeState::Idle)
        else {
            unreachable!("intake state checked above");
        };
        let outcome = pending.handle.await?;
        let failed = outcome.is_err();
        self.state = IntakeState::Ready(Box::new(ReadyTaskIntake {
            outcome,
            retry: pending.retry,
            value: pending.value,
            create_on_success: pending.create_on_success,
        }));
        Ok(IntakePoll::Ready { failed })
    }

    pub(super) fn cancel(&mut self) {
        if let IntakeState::Pending(pending) = std::mem::replace(&mut self.state, IntakeState::Idle)
        {
            pending.handle.abort();
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        !matches!(self.state, IntakeState::Idle)
    }

    pub(super) fn config(&self) -> &AppConfig {
        &self.config
    }

    pub(super) fn db_path(&self) -> Option<&std::path::Path> {
        self.db_path.as_deref()
    }

    pub(super) fn view(&self) -> IntakeViewState<'_> {
        IntakeViewState {
            add_task_only: self.add_task_only,
            message: self.message.as_deref(),
            work: match self.state {
                IntakeState::Idle => IntakeWorkView::Idle,
                IntakeState::Pending(_) => IntakeWorkView::Pending,
                IntakeState::Ready(_) => IntakeWorkView::Ready,
            },
        }
    }

    pub(super) fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    pub(super) fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }
}

impl Drop for IntakeController {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct DropSignal(Arc<Mutex<bool>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = true;
        }
    }

    fn draft(title: &str) -> TaskIntakeResult {
        TaskIntakeResult {
            task: crate::operations::TaskDraft {
                title: title.to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: crate::choices::TaskSource::Tui,
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
            recurrence: None,
        }
    }

    #[tokio::test]
    async fn poll_exposes_ready_before_completion() {
        let mut controller = IntakeController::new();
        controller.start_worker(
            tokio::spawn(async { Ok(draft("ready")) }),
            NaturalRetry::Dialog,
            "raw input".to_string(),
            false,
        );

        while matches!(controller.view().work, IntakeWorkView::Pending) {
            if matches!(controller.poll().await.unwrap(), IntakePoll::Ready { .. }) {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert_eq!(controller.view().work, IntakeWorkView::Ready);
        let IntakePoll::Completed(completion) = controller.poll().await.unwrap() else {
            panic!("ready intake should complete on the next poll");
        };
        assert_eq!(completion.outcome.unwrap().task.title, "ready");
        assert_eq!(completion.retry, NaturalRetry::Dialog);
        assert_eq!(completion.value, "raw input");
        assert!(!completion.create_on_success);
        assert!(!controller.work_pending());
    }

    #[tokio::test]
    async fn starting_new_work_cancels_active_worker() {
        let dropped = Arc::new(Mutex::new(false));
        let signal = DropSignal(Arc::clone(&dropped));
        let mut controller = IntakeController::new();
        controller.start_worker(
            tokio::spawn(async move {
                let _signal = signal;
                std::future::pending::<Result<TaskIntakeResult>>().await
            }),
            NaturalRetry::AddTask,
            "first".to_string(),
            false,
        );
        tokio::task::yield_now().await;

        controller.start_worker(
            tokio::spawn(async { Ok(draft("second")) }),
            NaturalRetry::Dialog,
            "second".to_string(),
            false,
        );
        for _ in 0..100 {
            if *dropped.lock().unwrap() {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(*dropped.lock().unwrap());
        assert!(controller.work_pending());
    }

    #[tokio::test]
    async fn cancel_aborts_active_worker_and_clears_work() {
        let dropped = Arc::new(Mutex::new(false));
        let signal = DropSignal(Arc::clone(&dropped));
        let mut controller = IntakeController::new();
        controller.start_worker(
            tokio::spawn(async move {
                let _signal = signal;
                std::future::pending::<Result<TaskIntakeResult>>().await
            }),
            NaturalRetry::AddTask,
            "pending".to_string(),
            false,
        );
        tokio::task::yield_now().await;

        controller.cancel();
        for _ in 0..100 {
            if *dropped.lock().unwrap() {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(*dropped.lock().unwrap());
        assert!(!controller.work_pending());
        assert_eq!(controller.view().work, IntakeWorkView::Idle);
    }
}

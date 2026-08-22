use anyhow::{Context, Result};

use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};
use crate::tui::app::{App, Notification};
use crate::tui::custom_command::{plan_invocation, resolve_command_targets};
use crate::tui::event::{CommandCatalog, CommandHandler};
use crate::tui::platform::TerminalTransition;
use crate::tui::terminal_command::{
    SystemTerminalProcessRunner, TerminalProcessRunner, execute_terminal_invocation_with,
};
use crate::tui::toast::ToastSeverity;

pub(crate) const REFRESH_ERROR_CHAR_LIMIT: usize = 512;

impl App {
    pub(super) async fn execute_command_handler(&mut self, handler: CommandHandler) -> Result<()> {
        if let CommandHandler::BuiltIn(action) = handler
            && action != crate::tui::event::Action::BeginCommand
            && matches!(
                self.store.view_state.query,
                crate::tui::store::TaskQuery::Recurring
                    | crate::tui::store::TaskQuery::RecentActions
            )
        {
            return self.execute(action).await;
        }
        let recurrence_series_id = self
            .selected_recurrence_target_id()
            .map(|target| target.series_id);
        let snapshot = self.capture_command_session(recurrence_series_id);
        match handler {
            CommandHandler::BuiltIn(action) => {
                let command = crate::tui::event::COMMANDS
                    .iter()
                    .find(|command| command.action == action)
                    .context("built-in command disappeared from the catalog")?;
                let resolved = match self.resolve_builtin_command(&snapshot, command).await? {
                    Ok(resolved) => resolved,
                    Err(reason) => {
                        self.set_warning(reason);
                        return Ok(());
                    }
                };
                self.execute_resolved_builtin(resolved, &snapshot).await
            }
            CommandHandler::Custom(command_id) => {
                let catalog = self.command_catalog.clone();
                let name = catalog
                    .custom(command_id)
                    .context("custom command disappeared from the catalog")?
                    .name
                    .clone();
                self.execute_captured_custom_command(&catalog, command_id, &name, &snapshot)
                    .await
            }
        }
    }

    pub(super) async fn execute_captured_custom_command(
        &mut self,
        catalog: &CommandCatalog,
        command_id: usize,
        invoked_as: &str,
        snapshot: &crate::tui::event::CommandSessionSnapshot,
    ) -> Result<()> {
        let command = catalog
            .custom(command_id)
            .cloned()
            .context("custom command disappeared from the catalog")?;
        if snapshot.workspace.id != self.store.active_workspace.id {
            self.set_warning(format!(
                ":{} is disabled: captured workspace is no longer active",
                command.name
            ));
            return Ok(());
        }
        let mut task_ids = snapshot
            .primary_task_id()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        task_ids.extend(snapshot.marked_task_ids().iter().cloned());
        let items = self.store.load_task_items(&task_ids).await?;
        if items.len() != task_ids.len() {
            self.set_warning(format!(
                ":{} is disabled: a captured task is stale",
                command.name
            ));
            return Ok(());
        }
        let primary = snapshot
            .primary_task_id()
            .and_then(|task_id| items.iter().find(|item| item.task.id == *task_id).cloned());
        let marked_items = snapshot
            .marked_task_ids()
            .iter()
            .filter_map(|task_id| items.iter().find(|item| item.task.id == *task_id).cloned())
            .collect::<Vec<_>>();
        let marked = marked_items.iter().collect::<Vec<_>>();
        let targets = match resolve_command_targets(command.target, primary.as_ref(), &marked) {
            Ok(targets) => targets,
            Err(reason) => {
                self.set_warning(format!(":{} is disabled: {reason}", command.name));
                return Ok(());
            }
        };
        let workspace = crate::workspaces::Workspace {
            id: snapshot.workspace.id.clone(),
            key: snapshot.workspace.key.clone(),
            name: snapshot.workspace.name.clone(),
        };
        let invocation = plan_invocation(
            &command,
            invoked_as,
            &workspace,
            primary.as_ref(),
            &marked,
            &targets,
            &self.custom_command_planning,
        )?;
        self.launch_custom_invocation(&command, invocation);
        Ok(())
    }

    fn launch_custom_invocation(
        &mut self,
        command: &crate::config::CustomTuiCommandConfig,
        invocation: crate::tui::custom_command::CustomCommandInvocation,
    ) {
        self.overlay = None;
        match command.execution {
            CustomTuiCommandExecution::Background => {
                if let Err(error) = self.custom_commands.launch(invocation) {
                    self.set_error(format!("{error:#}"));
                    return;
                }
                self.set_info(format!("launched :{}", command.name));
            }
            CustomTuiCommandExecution::Wait => {
                if let Err(error) = self.custom_commands.launch(invocation) {
                    self.set_error(format!("{error:#}"));
                    return;
                }
                self.notification =
                    Some(Notification::loading(format!("running :{}", command.name)));
            }
            CustomTuiCommandExecution::Terminal => {
                self.notification =
                    Some(Notification::loading(format!("opening :{}", command.name)));
                self.pending_terminal_command = Some(invocation);
            }
        }
    }

    pub(super) async fn execute_pending_terminal_command<T: TerminalTransition>(
        &mut self,
        transition: &mut T,
    ) -> bool {
        self.execute_pending_terminal_command_with(transition, &mut SystemTerminalProcessRunner)
            .await
    }

    pub(super) async fn execute_pending_terminal_command_with<
        T: TerminalTransition,
        R: TerminalProcessRunner,
    >(
        &mut self,
        transition: &mut T,
        runner: &mut R,
    ) -> bool {
        let Some(invocation) = self.pending_terminal_command.take() else {
            return false;
        };
        let name = invocation.name.clone();
        let on_success = invocation.on_success;
        let result = execute_terminal_invocation_with(transition, &invocation, runner).await;
        self.apply_custom_command_result(&name, on_success, result)
            .await;
        true
    }

    async fn apply_custom_command_result(
        &mut self,
        name: &str,
        on_success: CustomTuiCommandSuccess,
        result: std::result::Result<(), String>,
    ) {
        let Err(error) = result else {
            match on_success {
                CustomTuiCommandSuccess::Stay => self.set_info(completion_message(name)),
                CustomTuiCommandSuccess::Refresh => match self.refresh().await {
                    Ok(()) => self.set_info(completion_message(name)),
                    Err(error) => self.set_error(refresh_error_message(name, &error)),
                },
                CustomTuiCommandSuccess::Quit => self.should_quit = true,
                CustomTuiCommandSuccess::RefreshAndQuit => match self.refresh().await {
                    Ok(()) => self.should_quit = true,
                    Err(error) => self.set_error(refresh_error_message(name, &error)),
                },
            }
            return;
        };
        self.set_error(error);
    }

    pub(super) async fn poll_custom_commands(&mut self) -> bool {
        let completions = self.custom_commands.poll().await;
        if completions.is_empty() {
            return false;
        }
        let prior_error = self.notification.as_ref().and_then(|notification| {
            matches!(
                notification,
                Notification::Toast { toast, .. } if toast.severity == ToastSeverity::Error
            )
            .then(|| notification.clone())
        });
        let mut completion_info = None;
        let mut completion_error = None;
        for completion in completions {
            match completion.result {
                Err(error) => {
                    completion_error.get_or_insert(error);
                }
                Ok(()) => match completion.on_success {
                    CustomTuiCommandSuccess::Stay => {
                        completion_info = Some(completion_message(&completion.name));
                    }
                    CustomTuiCommandSuccess::Refresh => match self.refresh().await {
                        Ok(()) => completion_info = Some(completion_message(&completion.name)),
                        Err(error) => {
                            completion_error.get_or_insert_with(|| {
                                refresh_error_message(&completion.name, &error)
                            });
                        }
                    },
                    CustomTuiCommandSuccess::Quit => self.should_quit = true,
                    CustomTuiCommandSuccess::RefreshAndQuit => match self.refresh().await {
                        Ok(()) => self.should_quit = true,
                        Err(error) => {
                            completion_error.get_or_insert_with(|| {
                                refresh_error_message(&completion.name, &error)
                            });
                        }
                    },
                },
            }
        }
        if let Some(error) = completion_error {
            self.set_error(error);
        } else if let Some(error) = prior_error {
            self.notification = Some(error);
        } else if let Some(info) = completion_info {
            self.set_info(info);
        }
        true
    }
}

fn completion_message(name: &str) -> String {
    format!("custom command :{name} completed")
}

pub(crate) fn refresh_error_message(name: &str, error: &anyhow::Error) -> String {
    format!(
        ":{name} completed, but Aven could not refresh: {}",
        bounded_refresh_error(error)
    )
}

fn bounded_refresh_error(error: &anyhow::Error) -> String {
    let reason = format!("{error:#}")
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if reason.chars().count() <= REFRESH_ERROR_CHAR_LIMIT {
        return reason;
    }
    let mut bounded = reason
        .chars()
        .take(REFRESH_ERROR_CHAR_LIMIT.saturating_sub(1))
        .collect::<String>();
    bounded.push('…');
    bounded
}

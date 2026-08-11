use anyhow::{Context, Result};

use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};
use crate::tui::app::{App, Notification};
use crate::tui::custom_command::{plan_invocation, resolve_command_targets};
use crate::tui::event::CommandHandler;
use crate::tui::platform::TerminalTransition;
use crate::tui::terminal_command::{
    SystemTerminalProcessRunner, TerminalProcessRunner, execute_terminal_invocation_with,
};
use crate::tui::toast::ToastSeverity;

pub(crate) const REFRESH_ERROR_CHAR_LIMIT: usize = 512;

impl App {
    pub(super) async fn execute_command_handler(&mut self, handler: CommandHandler) -> Result<()> {
        match handler {
            CommandHandler::BuiltIn(action) => self.execute(action).await,
            CommandHandler::Custom(command_id) => {
                let name = self
                    .command_catalog
                    .custom(command_id)
                    .context("custom command disappeared from the catalog")?
                    .name
                    .clone();
                self.execute_custom_command(command_id, &name).await
            }
        }
    }

    pub(super) async fn execute_custom_command(
        &mut self,
        command_id: usize,
        invoked_as: &str,
    ) -> Result<()> {
        let command = self
            .command_catalog
            .custom(command_id)
            .cloned()
            .context("custom command disappeared from the catalog")?;
        let primary = self.store.selected_task(self.list.selected_task());
        let marked_ids = self.marked_task_ids_in_view();
        let marked = marked_ids
            .iter()
            .filter_map(|id| self.store.tasks.iter().find(|item| item.task.id == *id))
            .collect::<Vec<_>>();
        let targets = match resolve_command_targets(command.target, primary, &marked) {
            Ok(targets) => targets,
            Err(reason) => {
                self.set_warning(format!(":{} is disabled: {reason}", command.name));
                return Ok(());
            }
        };
        let invocation = plan_invocation(
            &command,
            invoked_as,
            &self.store.active_workspace,
            primary,
            &marked,
            &targets,
            &self.custom_command_planning,
        )?;
        self.overlay = None;
        match command.execution {
            CustomTuiCommandExecution::Background => {
                if let Err(error) = self.custom_commands.launch(invocation) {
                    self.set_error(format!("{error:#}"));
                    return Ok(());
                }
                self.set_info(format!("launched :{}", command.name));
            }
            CustomTuiCommandExecution::Wait => {
                if let Err(error) = self.custom_commands.launch(invocation) {
                    self.set_error(format!("{error:#}"));
                    return Ok(());
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
        Ok(())
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
